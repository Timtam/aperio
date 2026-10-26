//! Who the connected account is, in the calendar server's own terms.
//!
//! RFC 6638 names the account by every href of its principal's
//! `calendar-user-address-set`: a `mailto:`, a principal path, a `urn:`. A
//! server writes whichever it likes into ORGANIZER and ATTENDEE. iCloud, for
//! one, writes a principal path with the address in an EMAIL parameter:
//!
//! ```text
//! ORGANIZER;CN=Toni;EMAIL=toni@example.org:/aB1/principal/
//! ```
//!
//! (live measurement M1, 2026-09-19). Comparing with the `mailto:` alone
//! read the account's own meetings as someone else's. [`OwnIdentity::names`]
//! compares exactly, with only the equivalences each form defines: no
//! guessing, and an href the server did not list never counts.

use cal_core::attendee::normalize_address;
use url::Url;

/// The account's calendar-user addresses, ready to compare.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OwnIdentity {
    /// Addresses of the `mailto:` hrefs, normalised.
    mailtos: Vec<String>,
    /// Path and http(s) hrefs, resolved against the principal.
    urls: Vec<Url>,
    /// `urn:` hrefs, lower-cased.
    urns: Vec<String>,
}

impl OwnIdentity {
    /// Built from the hrefs of `calendar-user-address-set`. `principal` is
    /// the principal's URL, which relative hrefs are resolved against.
    pub fn from_hrefs(hrefs: &[String], principal: &Url) -> Self {
        let mut own = Self::default();
        for href in hrefs {
            let href = href.trim();
            if let Some(address) = strip_scheme(href, "mailto:") {
                let address = normalize_address(address);
                if !address.is_empty() && !own.mailtos.contains(&address) {
                    own.mailtos.push(address);
                }
            } else if starts_with_ci(href, "urn:") {
                own.urns.push(href.to_ascii_lowercase());
            } else if let Ok(url) = principal.join(href) {
                own.urls.push(url);
            }
        }
        own
    }

    /// Whether the server reported no address at all.
    pub fn is_empty(&self) -> bool {
        self.mailtos.is_empty() && self.urls.is_empty() && self.urns.is_empty()
    }

    /// Whether a calendar-user address names the account. `value` is a
    /// property value (`mailto:…`, a path, a URL, a `urn:`) or a bare
    /// address as Aperio shows it; `email` is the property's EMAIL parameter,
    /// the server's own statement of the address behind a non-mail value.
    pub fn names(&self, value: &str, email: Option<&str>) -> bool {
        if email.is_some_and(|e| self.has_address(e)) {
            return true;
        }
        let value = value.trim();
        if let Some(address) = strip_scheme(value, "mailto:") {
            return self.has_address(address);
        }
        if starts_with_ci(value, "urn:") {
            return self.urns.contains(&value.to_ascii_lowercase());
        }
        if value.starts_with('/')
            || starts_with_ci(value, "http:")
            || starts_with_ci(value, "https:")
        {
            return self.urls.iter().any(|own| {
                own.join(value)
                    .ok()
                    .is_some_and(|candidate| same_location(own, &candidate))
            });
        }
        // A bare address, the way the read side shows a mail value.
        value.contains('@') && self.has_address(value)
    }

    /// The account's mail addresses, normalised.
    pub fn mail_addresses(&self) -> &[String] {
        &self.mailtos
    }

    fn has_address(&self, address: &str) -> bool {
        let address = normalize_address(address);
        !address.is_empty() && self.mailtos.contains(&address)
    }
}

/// The same scheme, host and port, and the same path apart from one trailing
/// slash. `Url` already lower-cases the scheme and the host.
fn same_location(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
        && a.path().trim_end_matches('/') == b.path().trim_end_matches('/')
}

fn starts_with_ci(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn strip_scheme<'a>(value: &'a str, scheme: &str) -> Option<&'a str> {
    starts_with_ci(value, scheme).then(|| &value[scheme.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape measured live (M1): two principal paths, one without its
    /// trailing slash, a urn and the mail address.
    fn icloud() -> OwnIdentity {
        OwnIdentity::from_hrefs(
            &[
                "/1974/principal".into(),
                "/aB1/principal/".into(),
                "urn:uuid:1974".into(),
                "mailto:Toni@Example.org".into(),
            ],
            &Url::parse("https://p42-caldav.icloud.com/1974/principal/").unwrap(),
        )
    }

    #[test]
    fn every_listed_href_names_the_account() {
        let own = icloud();
        assert!(own.names("/aB1/principal/", None));
        assert!(own.names("/aB1/principal", None), "one trailing slash");
        assert!(own.names("/1974/principal/", None));
        assert!(own.names("https://P42-CALDAV.icloud.com:443/aB1/principal/", None));
        assert!(own.names("URN:UUID:1974", None));
        assert!(own.names("MAILTO:toni@example.ORG", None));
        assert!(
            own.names("toni@example.org", None),
            "the address Aperio shows"
        );
        assert_eq!(own.mail_addresses(), ["toni@example.org"]);
    }

    #[test]
    fn nothing_else_does() {
        let own = icloud();
        assert!(
            !own.names("/zZ9/principal/", None),
            "someone else's principal"
        );
        assert!(!own.names("https://other.example/aB1/principal/", None));
        assert!(!own.names("mailto:toni@example.org.evil", None));
        assert!(!own.names("urn:uuid:19740", None));
        assert!(!own.names("", None));
        assert!(OwnIdentity::default().is_empty());
        assert!(!OwnIdentity::default().names("mailto:toni@example.org", None));
    }

    #[test]
    fn the_email_parameter_counts_only_by_equality() {
        let own = icloud();
        assert!(own.names("/zZ9/principal/", Some("toni@example.org")));
        assert!(!own.names("/zZ9/principal/", Some("toni@example.org.evil")));
    }
}
