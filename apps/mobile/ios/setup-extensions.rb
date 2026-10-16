#!/usr/bin/env ruby
# One-time App Store Connect setup for the iPhone app's widget extension:
# the extension's bundle id, and the App Groups capability on it and on
# the app (so both can read `group.app.fasttrackstudio.task`, where the app
# writes what the widgets show). Idempotent: re-running changes nothing
# that is already in place.
#
#   source ~/.appstoreconnect/config.env && ruby setup-extensions.rb
#
# The App Group identifier itself is registered in the developer portal
# (Certificates, Identifiers & Profiles → Identifiers → App Groups); the
# public API has no resource for it. The script checks for it and says
# so when it is missing.

require "openssl"
require "json"
require "base64"
require "net/http"
require "uri"

APP_ID = ENV["APP_BUNDLE_ID"] || "app.fasttrackstudio.task"
WIDGETS_ID = ENV["WIDGETS_BUNDLE_ID"] || "#{APP_ID}.widgets"
GROUP_ID = ENV["APP_GROUP_ID"] || "group.#{APP_ID}"

KEY_ID = ENV.fetch("ASC_KEY_ID")
ISSUER_ID = ENV.fetch("ASC_ISSUER_ID")
KEY_PATH = ENV.fetch("ASC_KEY_PATH")

def jwt
  header = { alg: "ES256", kid: KEY_ID, typ: "JWT" }
  now = Time.now.to_i
  payload = { iss: ISSUER_ID, iat: now, exp: now + 1000, aud: "appstoreconnect-v1" }
  seg = ->(h) { Base64.urlsafe_encode64(JSON.dump(h), padding: false) }
  signing_input = "#{seg.call(header)}.#{seg.call(payload)}"
  key = OpenSSL::PKey::EC.new(File.read(KEY_PATH))
  der = key.sign(OpenSSL::Digest.new("SHA256"), signing_input)
  asn1 = OpenSSL::ASN1.decode(der)
  r = asn1.value[0].value.to_s(2).rjust(32, "\x00")
  s = asn1.value[1].value.to_s(2).rjust(32, "\x00")
  "#{signing_input}.#{Base64.urlsafe_encode64(r + s, padding: false)}"
end

TOKEN = jwt
BASE = "https://api.appstoreconnect.apple.com"

# Returns [status, parsed body]; never aborts, so each step can report.
def api(method, path, body = nil)
  uri = URI("#{BASE}#{path}")
  req = { get: Net::HTTP::Get, post: Net::HTTP::Post }.fetch(method).new(uri)
  req["Authorization"] = "Bearer #{TOKEN}"
  req["Content-Type"] = "application/json"
  req.body = JSON.dump(body) if body
  res = Net::HTTP.start(uri.host, uri.port, use_ssl: true) { |h| h.request(req) }
  parsed = (res.body.nil? || res.body.empty?) ? {} : (JSON.parse(res.body) rescue { "raw" => res.body })
  [res.code.to_i, parsed]
end

def bundle(identifier, name)
  _, list = api(:get, "/v1/bundleIds?filter[identifier]=#{identifier}&limit=200")
  found = (list["data"] || []).find { |b| b["attributes"]["identifier"] == identifier }
  return found if found
  code, made = api(:post, "/v1/bundleIds", {
    data: { type: "bundleIds", attributes: { identifier: identifier, name: name, platform: "IOS" } },
  })
  abort("create bundle id #{identifier} -> #{code}\n#{made}") unless code.between?(200, 299)
  puts "created bundle id #{identifier}"
  made["data"]
end

def capability(bundle, type)
  _, caps = api(:get, "/v1/bundleIds/#{bundle["id"]}/bundleIdCapabilities")
  if (caps["data"] || []).any? { |c| c["attributes"]["capabilityType"] == type }
    puts "#{bundle["attributes"]["identifier"]}: #{type} already on"
    return
  end
  code, body = api(:post, "/v1/bundleIdCapabilities", {
    data: {
      type: "bundleIdCapabilities",
      attributes: { capabilityType: type },
      relationships: { bundleId: { data: { type: "bundleIds", id: bundle["id"] } } },
    },
  })
  if code.between?(200, 299)
    puts "#{bundle["attributes"]["identifier"]}: #{type} on"
  else
    puts "#{bundle["attributes"]["identifier"]}: #{type} -> #{code} #{body.dig("errors", 0, "detail")}"
  end
end

app = bundle(APP_ID, "Task")
widgets = bundle(WIDGETS_ID, "Task Widgets")
[app, widgets].each { |b| capability(b, "APP_GROUPS") }

# Is the group registered? (No public resource: probe, and say so.)
code, _ = api(:get, "/v1/appGroups?filter[identifier]=#{GROUP_ID}")
puts(code == 200 ? "APP_GROUP_LISTABLE=yes" : "APP_GROUP_LISTABLE=no (#{code})")
puts "WIDGETS_BUNDLE_ID=#{WIDGETS_ID}"
puts "APP_GROUP_ID=#{GROUP_ID}"
