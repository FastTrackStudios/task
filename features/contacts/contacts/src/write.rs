//! [`Contact`] → markdown.
//!
//! Structured (vCard-mapped) fields become YAML frontmatter (with a
//! `contact` `type:` discriminator so vault tooling can recognize the
//! file); the multi-valued fields (emails / phones / groups) are emitted
//! as YAML **sequences** for readability, even though the entity keeps
//! them newline-joined. The free-form notes become the markdown body.

use contacts_proto::Contact;

pub use vault_entity::WriteError;

fn seq(values: Vec<&str>) -> serde_yaml::Value {
    serde_yaml::Value::Sequence(
        values
            .into_iter()
            .map(|v| serde_yaml::Value::from(v.to_string()))
            .collect(),
    )
}

pub fn serialize_contact(contact: &Contact) -> Result<String, WriteError> {
    let mut map = serde_yaml::Mapping::new();
    map.insert("type".into(), "contact".into());
    map.insert("id".into(), contact.id.clone().into());
    if let Some(uid) = &contact.uid {
        map.insert("uid".into(), uid.clone().into());
    }
    map.insert("full_name".into(), contact.full_name.clone().into());
    if let Some(v) = &contact.given_name {
        map.insert("given_name".into(), v.clone().into());
    }
    if let Some(v) = &contact.family_name {
        map.insert("family_name".into(), v.clone().into());
    }
    if let Some(v) = &contact.organization {
        map.insert("organization".into(), v.clone().into());
    }
    if let Some(v) = &contact.title {
        map.insert("title".into(), v.clone().into());
    }
    if !contact.emails.trim().is_empty() {
        map.insert("emails".into(), seq(contact.email_list()));
    }
    if !contact.phones.trim().is_empty() {
        map.insert("phones".into(), seq(contact.phone_list()));
    }
    if let Some(v) = &contact.address {
        map.insert("address".into(), v.clone().into());
    }
    if let Some(v) = &contact.birthday {
        map.insert("birthday".into(), v.clone().into());
    }
    if let Some(v) = &contact.photo_url {
        map.insert("photo_url".into(), v.clone().into());
    }
    if !contact.groups.trim().is_empty() {
        map.insert("groups".into(), seq(contact.group_list()));
    }
    map.insert("source".into(), contact.source.clone().into());
    if let Some(v) = &contact.account {
        map.insert("account".into(), v.clone().into());
    }
    if let Some(v) = &contact.etag {
        map.insert("etag".into(), v.clone().into());
    }
    if let Some(v) = &contact.linked_party_id {
        map.insert("linked_party_id".into(), v.clone().into());
    }
    if let Some(v) = &contact.linked_user_id {
        map.insert("linked_user_id".into(), v.clone().into());
    }
    map.insert("archived".into(), contact.archived.into());
    map.insert("created".into(), contact.created.clone().into());
    if let Some(v) = &contact.updated {
        map.insert("updated".into(), v.clone().into());
    }

    let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(map))
        .map_err(|e| WriteError::Yaml(e.to_string()))?;
    let notes = contact.notes.clone().unwrap_or_default();
    Ok(format!("---\n{yaml}---\n\n{notes}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{frontmatter_split, parse_contact};

    #[test]
    fn round_trip_full_contact() {
        let mut c = Contact::create("abc-123", "Ada Lovelace", "2026-07-17T09:00:00Z");
        c.uid = Some("urn:uuid:xyz".into());
        c.given_name = Some("Ada".into());
        c.family_name = Some("Lovelace".into());
        c.organization = Some("Analytical Engine Co".into());
        c.title = Some("Programmer".into());
        c.emails = "ada@example.com\nada.l@work.com".into();
        c.phones = "+1 555 0100".into();
        c.address = Some("12 Bayswater\nLondon".into());
        c.groups = "Engineering\nFriends".into();
        c.notes = Some("Met at the Analytical Society.".into());
        c.source = contacts_proto::ContactSource::ICLOUD.to_string();
        c.account = Some("iCloud".into());
        c.etag = Some("\"abc\"".into());
        c.linked_party_id = Some("party-1".into());

        let page = serialize_contact(&c).unwrap();
        let (fm, body) = frontmatter_split(&page).expect("frontmatter");
        let parsed = parse_contact("Records/contacts/abc-123.md", fm, body).expect("parse");
        assert_eq!(parsed, c);
    }

    #[test]
    fn minimal_contact_round_trips() {
        let c = Contact::create("min", "Nobody", "2026-07-17T09:00:00Z");
        let page = serialize_contact(&c).unwrap();
        let (fm, body) = frontmatter_split(&page).expect("frontmatter");
        let parsed = parse_contact("Records/contacts/min.md", fm, body).expect("parse");
        assert_eq!(parsed, c);
        assert!(parsed.email_list().is_empty());
    }
}
