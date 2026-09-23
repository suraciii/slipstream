# Local HTTPS test certificates

These public test credentials serve synthetic browser and CLI fixtures only.
`cert.pem` is the trusted test CA; `server-cert.pem` and `server-key.pem` are its
localhost/127.0.0.1 server certificate and private key. Never use this key for a
real deployment. Test clients explicitly select this CA; production TLS
verification is unchanged. The fixtures create fresh instance Access Tokens in
private temporary files and never use these certificates as Library credentials.
