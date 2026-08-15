// HTTP Digest auth (RFC 2617): challenge parsing and the Authorization
// header value, qop="auth" and legacy no-qop.

use crate::md5::md5_hex;

#[derive(Debug, Default, Clone)]
pub struct DigestChallenge {
    pub realm: String,
    pub nonce: String,
    pub qop: Option<String>,
    pub opaque: Option<String>,
}

pub fn parse_challenge(www_authenticate: &str) -> Option<DigestChallenge> {
    let rest = www_authenticate.trim().strip_prefix("Digest")?.trim();
    let mut ch = DigestChallenge::default();
    // split on commas outside quotes
    let mut items = Vec::new();
    let (mut depth_q, mut start) = (false, 0usize);
    let bytes = rest.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' => depth_q = !depth_q,
            b',' if !depth_q => {
                items.push(&rest[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    items.push(&rest[start..]);
    for it in items {
        let mut kv = it.splitn(2, '=');
        let k = kv.next()?.trim().to_ascii_lowercase();
        let v = kv.next().unwrap_or("").trim().trim_matches('"').to_string();
        match k.as_str() {
            "realm" => ch.realm = v,
            "nonce" => ch.nonce = v,
            "qop" => ch.qop = Some(v),
            "opaque" => ch.opaque = Some(v),
            _ => {}
        }
    }
    if ch.nonce.is_empty() {
        return None;
    }
    Some(ch)
}

/// Build the Authorization header value. `cnonce`/`nc` are injectable for tests.
pub fn digest_authorization(
    ch: &DigestChallenge,
    user: &str,
    pass: &str,
    method: &str,
    uri: &str,
    cnonce: &str,
    nc: u32,
) -> String {
    let ha1 = md5_hex(format!("{user}:{}:{pass}", ch.realm).as_bytes());
    let ha2 = md5_hex(format!("{method}:{uri}").as_bytes());
    let nc_s = format!("{:08x}", nc);
    let (response, qop_part) = match ch.qop.as_deref() {
        Some(q) if q.split(',').any(|x| x.trim() == "auth") => {
            let r = md5_hex(
                format!("{ha1}:{}:{nc_s}:{cnonce}:auth:{ha2}", ch.nonce).as_bytes(),
            );
            (r, format!(", qop=auth, nc={nc_s}, cnonce=\"{cnonce}\""))
        }
        _ => (
            md5_hex(format!("{ha1}:{}:{ha2}", ch.nonce).as_bytes()),
            String::new(),
        ),
    };
    let opaque = ch
        .opaque
        .as_ref()
        .map(|o| format!(", opaque=\"{o}\""))
        .unwrap_or_default();
    format!(
        "Digest username=\"{user}\", realm=\"{}\", nonce=\"{}\", uri=\"{uri}\", \
         algorithm=MD5, response=\"{response}\"{qop_part}{opaque}",
        ch.realm, ch.nonce
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_rfc2617_worked_example() {
        // RFC 2617 section 3.5: user "Mufasa", pass "Circle Of Life",
        // GET /dir/index.html, cnonce 0a4f113b, nc 1
        let ch = DigestChallenge {
            realm: "testrealm@host.com".into(),
            nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093".into(),
            qop: Some("auth".into()),
            opaque: Some("5ccc069c403ebaf9f0171e9517f40e41".into()),
        };
        let h = digest_authorization(
            &ch,
            "Mufasa",
            "Circle Of Life",
            "GET",
            "/dir/index.html",
            "0a4f113b",
            1,
        );
        assert!(
            h.contains("response=\"6629fae49393a05397450978507c4ef1\""),
            "got: {h}"
        );
        assert!(h.contains("qop=auth"));
        assert!(h.contains("nc=00000001"));
    }

    #[test]
    fn challenge_parser_handles_quoted_commas() {
        let ch = parse_challenge(
            "Digest realm=\"Printer, Admin\", nonce=\"abc\", qop=\"auth,auth-int\", opaque=\"xyz\"",
        )
        .unwrap();
        assert_eq!(ch.realm, "Printer, Admin");
        assert_eq!(ch.nonce, "abc");
        assert_eq!(ch.qop.as_deref(), Some("auth,auth-int"));
    }
}
