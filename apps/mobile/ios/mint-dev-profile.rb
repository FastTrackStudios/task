#!/usr/bin/env ruby
# Mint an iOS development provisioning profile via the App Store Connect API
# and install it, so `codesign`/dx can sign the app for a physical device.
#
# Registers the target device if needed, finds the team's Development
# certificate, ensures the bundle id exists, then creates (replacing any
# same-named) an IOS_APP_DEVELOPMENT profile covering every registered iOS
# device. Writes the .mobileprovision into Xcode's profiles dir and prints
# its path.
#
#   ruby mint-dev-profile.rb <device-udid> [bundle-id] [profile-name]
#
# Reads ASC_KEY_ID / ASC_ISSUER_ID / ASC_KEY_PATH from the environment
# (source ~/.appstoreconnect/config.env first).

require "openssl"
require "json"
require "base64"
require "net/http"
require "uri"
require "securerandom"

DEVICE_UDID = ARGV[0] or abort("usage: mint-dev-profile.rb <device-udid|-> [bundle-id] [name]")
BUNDLE_ID = ARGV[1] || "app.fasttrackstudio"
# Profile/cert type: development (device install) by default; set
# PROFILE_TYPE=IOS_APP_STORE + CERT_TYPE=DISTRIBUTION for a TestFlight
# upload profile (no device list needed there).
PROFILE_TYPE = ENV["PROFILE_TYPE"] || "IOS_APP_DEVELOPMENT"
CERT_TYPE = ENV["CERT_TYPE"] || "DEVELOPMENT"
APP_STORE = PROFILE_TYPE == "IOS_APP_STORE"
PROFILE_NAME = ARGV[2] || (APP_STORE ? "FTS App Store" : "FTS Dev")

KEY_ID = ENV.fetch("ASC_KEY_ID")
ISSUER_ID = ENV.fetch("ASC_ISSUER_ID")
KEY_PATH = ENV.fetch("ASC_KEY_PATH")

# ── JWT (ES256) ──────────────────────────────────────────────────────────
def jwt
  header = { alg: "ES256", kid: KEY_ID, typ: "JWT" }
  now = Time.now.to_i
  payload = { iss: ISSUER_ID, iat: now, exp: now + 1000, aud: "appstoreconnect-v1" }
  seg = ->(h) { Base64.urlsafe_encode64(JSON.dump(h), padding: false) }
  signing_input = "#{seg.call(header)}.#{seg.call(payload)}"
  key = OpenSSL::PKey::EC.new(File.read(KEY_PATH))
  der = key.sign(OpenSSL::Digest.new("SHA256"), signing_input)
  # DER ECDSA sig -> raw r||s (64 bytes) for JOSE.
  asn1 = OpenSSL::ASN1.decode(der)
  r = asn1.value[0].value.to_s(2).rjust(32, "\x00")
  s = asn1.value[1].value.to_s(2).rjust(32, "\x00")
  sig = Base64.urlsafe_encode64(r + s, padding: false)
  "#{signing_input}.#{sig}"
end

TOKEN = jwt
BASE = "https://api.appstoreconnect.apple.com"

def api(method, path, body = nil)
  uri = URI("#{BASE}#{path}")
  req = case method
        when :get then Net::HTTP::Get.new(uri)
        when :post then Net::HTTP::Post.new(uri)
        when :delete then Net::HTTP::Delete.new(uri)
        end
  req["Authorization"] = "Bearer #{TOKEN}"
  req["Content-Type"] = "application/json"
  req.body = JSON.dump(body) if body
  res = Net::HTTP.start(uri.host, uri.port, use_ssl: true) { |h| h.request(req) }
  unless res.code.to_i.between?(200, 299)
    abort("API #{method} #{path} -> #{res.code}\n#{res.body}")
  end
  (res.body.nil? || res.body.empty?) ? {} : JSON.parse(res.body)
end

# ── 1. Devices (development profiles only; App Store profiles carry none) ──
device_ids = []
unless APP_STORE
  devices = api(:get, "/v1/devices?filter[platform]=IOS&limit=200")["data"]
  dev = devices.find { |d| d["attributes"]["udid"].to_s.casecmp?(DEVICE_UDID) }
  if dev.nil?
    puts "registering device #{DEVICE_UDID}"
    dev = api(:post, "/v1/devices", {
      data: { type: "devices", attributes: {
        name: "FTS iPhone", platform: "IOS", udid: DEVICE_UDID
      } }
    })["data"]
  end
  device_ids = devices.select { |d| d["attributes"]["status"] == "ENABLED" }.map { |d| d["id"] }
  device_ids |= [dev["id"]]
  puts "devices in profile: #{device_ids.size}"
end

# ── 2. Signing certificate ───────────────────────────────────────────────
certs = api(:get, "/v1/certificates?filter[certificateType]=#{CERT_TYPE}&limit=50")["data"]
if certs.empty?
  hint = APP_STORE ? "Apple Distribution" : "Apple Development"
  abort("no #{CERT_TYPE} certificate — create an '#{hint}' cert in Xcode " \
        "(Settings → Accounts → Manage Certificates → +)")
end
cert_id = certs.first["id"]
puts "cert: #{certs.first["attributes"]["name"]}"

# ── 3. Bundle id (create if missing) ─────────────────────────────────────
# `filter[identifier]` is a substring match, not exact (so "app.foo" also
# returns "app.foo.rig") — re-check the identifier client-side.
bundle = api(:get, "/v1/bundleIds?filter[identifier]=#{BUNDLE_ID}&limit=200")["data"]
  .find { |b| b["attributes"]["identifier"] == BUNDLE_ID }
if bundle.nil?
  puts "creating bundle id #{BUNDLE_ID}"
  bundle = api(:post, "/v1/bundleIds", {
    data: { type: "bundleIds", attributes: {
      identifier: BUNDLE_ID, name: "FastTrackStudio", platform: "IOS"
    } }
  })["data"]
end
bundle_id = bundle["id"]

# ── 4. Profile (delete same-named, then create) ──────────────────────────
existing = api(:get, "/v1/profiles?filter[name]=#{URI.encode_www_form_component(PROFILE_NAME)}&limit=10")["data"]
existing.each do |p|
  puts "deleting stale profile #{p["id"]}"
  api(:delete, "/v1/profiles/#{p["id"]}")
end

relationships = {
  bundleId: { data: { type: "bundleIds", id: bundle_id } },
  certificates: { data: [{ type: "certificates", id: cert_id }] },
}
# App Store profiles carry no devices; development profiles do.
relationships[:devices] = { data: device_ids.map { |id| { type: "devices", id: id } } } unless APP_STORE

profile = api(:post, "/v1/profiles", {
  data: {
    type: "profiles",
    attributes: { name: PROFILE_NAME, profileType: PROFILE_TYPE },
    relationships: relationships,
  }
})["data"]

content = Base64.decode64(profile["attributes"]["profileContent"])
dir = File.expand_path("~/Library/Developer/Xcode/UserData/Provisioning Profiles")
require "fileutils"
FileUtils.mkdir_p(dir)
out = File.join(dir, "#{profile["attributes"]["uuid"]}.mobileprovision")
File.binwrite(out, content)
puts "PROFILE_PATH=#{out}"
puts "PROFILE_UUID=#{profile["attributes"]["uuid"]}"
