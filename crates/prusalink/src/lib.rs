// prusalink -- minimal client for the PrusaLink HTTP API on Buddy firmware.
//
// std-only: hand-rolled MD5 (RFC 1321), HTTP Digest (RFC 2617, qop="auth" and
// legacy no-qop), and a small HTTP/1.1 client over TcpStream. Auth is either
// X-Api-Key (the PrusaLink local key, NOT the PrusaConnect key) or Digest with
// the username/password from the printer's PrusaLink settings screen.
//
// Endpoints per prusa3d/Prusa-Link-Web spec/openapi.yaml:
//   GET    /api/v1/info | /api/v1/status | /api/v1/job | /api/v1/storage
//   PUT    /api/v1/files/{storage}/{path}   (Print-After-Upload / Overwrite: ?0|?1)
//   DELETE /api/v1/job/{id}
//
// One concern per module; everything public is re-exported here so consumer
// paths stay flat (`prusalink::Client`, `prusalink::md5_hex`, ...).

mod client;
mod digest;
mod http;
mod md5;

pub use client::{Client, LogFn, PrinterAuth};
pub use digest::{digest_authorization, parse_challenge, DigestChallenge};
pub use http::Response;
pub use md5::md5_hex;
