#!/usr/bin/env ruby
# Ensure an Apple Distribution signing certificate + its private key exist
# locally (App Store Connect API — no Xcode UI needed). Generates an RSA key
# + CSR, creates the cert via the API, and writes:
#   ~/.appstoreconnect/dist.key  (PEM private key)
#   ~/.appstoreconnect/dist.cer  (DER certificate)
# The caller (deploy-testflight.sh) bundles these into a .p12 and imports
# them into the login keychain so codesign can use them.
#
# Idempotent: if both files already exist and the cert is still valid in the
# account, reuses them. Reads ASC_* from the environment.

require "openssl"
require "json"
require "base64"
require "net/http"
require "uri"
require "fileutils"

KEY_ID = ENV.fetch("ASC_KEY_ID")
ISSUER_ID = ENV.fetch("ASC_ISSUER_ID")
KEY_PATH = ENV.fetch("ASC_KEY_PATH")
DIR = File.expand_path("~/.appstoreconnect")
DIST_KEY = File.join(DIR, "dist.key")
DIST_CER = File.join(DIR, "dist.cer")

def jwt
  header = { alg: "ES256", kid: KEY_ID, typ: "JWT" }
  now = Time.now.to_i
  payload = { iss: ISSUER_ID, iat: now, exp: now + 900, aud: "appstoreconnect-v1" }
  seg = ->(h) { Base64.urlsafe_encode64(JSON.dump(h), padding: false) }
  input = "#{seg.call(header)}.#{seg.call(payload)}"
  key = OpenSSL::PKey::EC.new(File.read(KEY_PATH))
  der = key.sign(OpenSSL::Digest.new("SHA256"), input)
  asn1 = OpenSSL::ASN1.decode(der)
  r = asn1.value[0].value.to_s(2).rjust(32, "\x00")
  s = asn1.value[1].value.to_s(2).rjust(32, "\x00")
  "#{input}.#{Base64.urlsafe_encode64(r + s, padding: false)}"
end

TOKEN = jwt

def api(method, path, body = nil)
  uri = URI("https://api.appstoreconnect.apple.com#{path}")
  req = (method == :post ? Net::HTTP::Post : Net::HTTP::Get).new(uri)
  req["Authorization"] = "Bearer #{TOKEN}"
  req["Content-Type"] = "application/json"
  req.body = JSON.dump(body) if body
  res = Net::HTTP.start(uri.host, uri.port, use_ssl: true) { |h| h.request(req) }
  abort("API #{method} #{path} -> #{res.code}\n#{res.body}") unless res.code.to_i.between?(200, 299)
  res.body.to_s.empty? ? {} : JSON.parse(res.body)
end

# Reuse if we already have a local key+cert AND a live DISTRIBUTION cert
# whose serial matches (Apple caps distribution certs, so don't spawn dupes).
if File.exist?(DIST_KEY) && File.exist?(DIST_CER)
  local = OpenSSL::X509::Certificate.new(File.binread(DIST_CER))
  live = api(:get, "/v1/certificates?filter[certificateType]=DISTRIBUTION&limit=50")["data"]
  if live.any? { |c| OpenSSL::X509::Certificate.new(Base64.decode64(c["attributes"]["certificateContent"])).serial == local.serial }
    puts "DIST_KEY=#{DIST_KEY}"
    puts "DIST_CER=#{DIST_CER}"
    exit
  end
end

FileUtils.mkdir_p(DIR)
key = OpenSSL::PKey::RSA.new(2048)
File.write(DIST_KEY, key.to_pem)
File.chmod(0o600, DIST_KEY)

csr = OpenSSL::X509::Request.new
csr.version = 0
csr.subject = OpenSSL::X509::Name.new([["CN", "FastTrackStudio Distribution"], ["C", "US"]])
csr.public_key = key.public_key
csr.sign(key, OpenSSL::Digest.new("SHA256"))

resp = api(:post, "/v1/certificates", {
  data: { type: "certificates", attributes: {
    certificateType: "DISTRIBUTION",
    # The API wants the CSR's PEM content verbatim (BEGIN/END + newlines),
    # not a re-base64 of it.
    csrContent: csr.to_pem,
  } }
})
File.binwrite(DIST_CER, Base64.decode64(resp["data"]["attributes"]["certificateContent"]))
puts "created Apple Distribution certificate"
puts "DIST_KEY=#{DIST_KEY}"
puts "DIST_CER=#{DIST_CER}"
