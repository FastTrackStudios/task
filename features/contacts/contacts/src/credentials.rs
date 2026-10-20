//! Where a CardDAV password lives — anywhere but the vault.
//!
//! An account file (`Records/contacts/.sync/<id>.md`) carries a
//! [`PasswordRef`], never the password. The reference is resolved at
//! sync time through a [`CredentialStore`]:
//!
//! - [`PasswordRef::Keyring`] — the OS keyring, through
//!   `email-secret`'s resolver (Secret Service / Keychain / Credential
//!   Manager / kernel keyutils). The default on a native install.
//! - [`PasswordRef::Envelope`] — the `auth::crypto::encrypt_secret`
//!   AEAD envelope, keyed by the server's auth secret. The fallback for
//!   a node with no usable keyring (a headless server, a container).
//! - [`PasswordRef::Test`] — a test-only source. Resolved only by
//!   [`TestSecrets`]; a production store refuses it, so a seeded or
//!   copied file naming one can never be mistaken for a credential.
//!
//! The account directory is also node-private (`files_proto`'s
//! `PRIVATE_MARKER`), so even the reference never reaches a peer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use contacts_proto::ContactsError;
use email_secret::{Secret, SecretValue};
use serde::{Deserialize, Serialize};

/// The keyring service every contacts credential is filed under.
pub const KEYRING_SERVICE: &str = "task-contacts";

/// What an account file says about its password: where to find it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PasswordRef {
    /// An OS keyring entry.
    Keyring { service: String, account: String },
    /// `auth::crypto::encrypt_secret` ciphertext (`v2.<keyid>.…`),
    /// decryptable only with the node's auth secret.
    Envelope { ciphertext: String },
    /// A test-only source ([`TestSecrets`]). Never resolved in
    /// production.
    Test { key: String },
}

/// Puts passwords somewhere safe and gets them back.
pub trait CredentialStore: Send + Sync + 'static {
    /// Keep `password` for the account `key` (unique per account and
    /// vault) and return the reference to write in its place.
    fn store(&self, key: &str, password: &str) -> Result<PasswordRef, ContactsError>;
    /// The password a reference names.
    fn resolve(&self, reference: &PasswordRef) -> Result<SecretValue, ContactsError>;
    /// Drop what a reference names, best-effort (an account deleted or
    /// its password replaced).
    fn forget(&self, reference: &PasswordRef);
}

/// Which store a native node prefers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preference {
    /// The OS keyring, falling back to the envelope when it refuses.
    Keyring,
    /// The envelope only.
    Envelope,
}

impl Preference {
    /// The platform default.
    ///
    /// The keyring everywhere it is durable. On Linux the `keyring`
    /// crate's native backend is kernel keyutils, which does **not**
    /// survive a reboot — a server that kept its CardDAV passwords there
    /// would lose every one of them on its next restart of the machine —
    /// so Linux defaults to the envelope. `TASK_CONTACTS_SECRETS`
    /// (`keyring` | `envelope`) overrides either way; see
    /// [`Preference::from_env`].
    #[must_use]
    pub fn platform_default() -> Self {
        if cfg!(target_os = "linux") {
            Self::Envelope
        } else {
            Self::Keyring
        }
    }

    /// `TASK_CONTACTS_SECRETS`, or the platform default.
    #[must_use]
    pub fn from_env() -> Self {
        match std::env::var("TASK_CONTACTS_SECRETS").as_deref() {
            Ok("keyring") => Self::Keyring,
            Ok("envelope") => Self::Envelope,
            _ => Self::platform_default(),
        }
    }
}

/// The production store: keyring first (when preferred), envelope as
/// the fallback.
pub struct NativeCredentials {
    preference: Preference,
    /// The auth secret the envelope is keyed by. `None` disables the
    /// envelope — a node with neither a keyring nor a key cannot keep a
    /// password, and says so rather than writing it in the clear.
    envelope_key: Option<zeroize::Zeroizing<String>>,
}

impl NativeCredentials {
    #[must_use]
    pub fn new(preference: Preference, envelope_key: Option<String>) -> Self {
        Self {
            preference,
            envelope_key: envelope_key.map(zeroize::Zeroizing::new),
        }
    }

    fn envelope(&self, password: &str) -> Result<PasswordRef, ContactsError> {
        let key = self
            .envelope_key
            .as_deref()
            .ok_or_else(|| ContactsError::Backend {
                message: "no secret store available: the keyring refused and no auth secret is \
                      configured for the envelope"
                    .into(),
            })?;
        let ciphertext =
            auth::crypto::encrypt_secret(key, password).map_err(|e| ContactsError::Backend {
                message: format!("encrypting the CardDAV password: {e}"),
            })?;
        Ok(PasswordRef::Envelope { ciphertext })
    }
}

impl CredentialStore for NativeCredentials {
    fn store(&self, key: &str, password: &str) -> Result<PasswordRef, ContactsError> {
        if self.preference == Preference::Keyring {
            let stored = email_secret::keyring_store(KEYRING_SERVICE, key, password)
                .and_then(|()| {
                    // Read it back: some backends accept a write they
                    // cannot serve (a locked collection, a mock).
                    Secret::keyring(KEYRING_SERVICE, key).resolve_blocking()
                })
                .is_ok_and(|v| v.as_str() == password);
            if stored {
                return Ok(PasswordRef::Keyring {
                    service: KEYRING_SERVICE.into(),
                    account: key.into(),
                });
            }
            tracing::warn!(
                key,
                "contacts: keyring unavailable for a CardDAV password; using the envelope"
            );
        }
        self.envelope(password)
    }

    fn resolve(&self, reference: &PasswordRef) -> Result<SecretValue, ContactsError> {
        match reference {
            PasswordRef::Keyring { service, account } => {
                Secret::keyring(service.clone(), account.clone())
                    .resolve_blocking()
                    .map_err(|e| ContactsError::Sync {
                        message: format!(
                            "the CardDAV password is not in the keyring ({e}); re-enter it"
                        ),
                    })
            }
            PasswordRef::Envelope { ciphertext } => {
                let key = self
                    .envelope_key
                    .as_deref()
                    .ok_or_else(|| ContactsError::Sync {
                        message: "the CardDAV password is sealed but no auth secret is configured"
                            .into(),
                    })?;
                auth::crypto::decrypt_secret(key, ciphertext)
                    .map(SecretValue::new)
                    .map_err(|e| ContactsError::Sync {
                        message: format!(
                            "the CardDAV password could not be unsealed ({e}); re-enter it"
                        ),
                    })
            }
            PasswordRef::Test { .. } => Err(ContactsError::Sync {
                message: "this account's password comes from a test-only source; re-enter it"
                    .into(),
            }),
        }
    }

    fn forget(&self, reference: &PasswordRef) {
        if let PasswordRef::Keyring { service, account } = reference
            && let Err(e) = email_secret::keyring_delete(service, account)
        {
            tracing::warn!(%e, "contacts: could not remove a CardDAV password from the keyring");
        }
    }
}

/// The test-only source: a process-wide map, so a "restart" (a new
/// `VaultContacts` over the same vault) still resolves what the old one
/// stored. Never used by a production build of the server.
#[derive(Clone, Default)]
pub struct TestSecrets;

fn test_map() -> &'static Mutex<HashMap<String, String>> {
    static MAP: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

impl TestSecrets {
    /// Plant `value` under `key` — what a seed's [`PasswordRef::Test`]
    /// names.
    pub fn plant(key: &str, value: &str) {
        test_map()
            .lock()
            .expect("test secrets poisoned")
            .insert(key.into(), value.into());
    }

    /// The store, shareable.
    #[must_use]
    pub fn shared() -> Arc<dyn CredentialStore> {
        Arc::new(Self)
    }
}

impl CredentialStore for TestSecrets {
    fn store(&self, key: &str, password: &str) -> Result<PasswordRef, ContactsError> {
        Self::plant(key, password);
        Ok(PasswordRef::Test { key: key.into() })
    }

    fn resolve(&self, reference: &PasswordRef) -> Result<SecretValue, ContactsError> {
        match reference {
            PasswordRef::Test { key } => test_map()
                .lock()
                .expect("test secrets poisoned")
                .get(key)
                .cloned()
                .map(SecretValue::new)
                .ok_or_else(|| ContactsError::Sync {
                    message: format!("no test secret planted under {key}"),
                }),
            other => Err(ContactsError::Sync {
                message: format!("the test store cannot resolve {other:?}"),
            }),
        }
    }

    fn forget(&self, reference: &PasswordRef) {
        if let PasswordRef::Test { key } = reference {
            test_map()
                .lock()
                .expect("test secrets poisoned")
                .remove(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "unit-test-auth-secret-at-least-32-bytes-long";

    #[test]
    fn the_envelope_round_trips_and_holds_no_plaintext() {
        let store = NativeCredentials::new(Preference::Envelope, Some(KEY.into()));
        let r = store.store("carddav:x:a1", "app-secret-123").unwrap();
        let PasswordRef::Envelope { ciphertext } = &r else {
            panic!("expected an envelope, got {r:?}");
        };
        assert!(!ciphertext.contains("app-secret-123"));
        assert_eq!(store.resolve(&r).unwrap().as_str(), "app-secret-123");

        // A node with another key cannot open it.
        let other = NativeCredentials::new(Preference::Envelope, Some(format!("{KEY}-other")));
        assert!(other.resolve(&r).is_err());
    }

    #[test]
    fn no_keyring_and_no_key_refuses_rather_than_storing_in_the_clear() {
        let store = NativeCredentials::new(Preference::Envelope, None);
        assert!(store.store("carddav:x:a1", "pw").is_err());
    }

    #[test]
    fn production_refuses_a_test_reference() {
        let store = NativeCredentials::new(Preference::Envelope, Some(KEY.into()));
        TestSecrets::plant("seeded", "pw");
        assert!(
            store
                .resolve(&PasswordRef::Test {
                    key: "seeded".into()
                })
                .is_err()
        );
    }

    /// The keyring path, where this machine has a usable keyring. On a
    /// host with none the store falls back to the envelope, which is
    /// the behaviour under test there.
    #[test]
    fn keyring_preference_round_trips_or_falls_back() {
        let store = NativeCredentials::new(Preference::Keyring, Some(KEY.into()));
        let key = format!("carddav:unit-test:{}", uuid::Uuid::new_v4());
        let r = store.store(&key, "kr-secret").unwrap();
        assert_eq!(store.resolve(&r).unwrap().as_str(), "kr-secret");
        store.forget(&r);
        if matches!(r, PasswordRef::Keyring { .. }) {
            assert!(store.resolve(&r).is_err(), "forget left the keyring entry");
        }
    }

    #[test]
    fn references_serialize_without_the_password() {
        let r = PasswordRef::Keyring {
            service: KEYRING_SERVICE.into(),
            account: "carddav:v:a1".into(),
        };
        let yaml = serde_yaml::to_string(&r).unwrap();
        assert!(yaml.contains("type: keyring"), "{yaml}");
        let back: PasswordRef = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back, r);
    }
}
