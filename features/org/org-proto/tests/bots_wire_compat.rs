//! `CreateBotRequest` stays decodable from apps shipped before 0.2.
//!
//! Desktop builds up to desktop-v0.0.5 send `{session_token, name, orgs,
//! tools}` with no `no_orgs`. vox decodes a call's args through a
//! writer→reader compat program, and a field only the reader has is
//! allowed only when it has a default. These tests run the same program
//! (`vox_phon::build_decode_program`) over the old shape's bytes.

use org_proto::{BotTools, CreateBotRequest};

/// `CreateBotRequest` as shipped desktop builds still send it.
mod legacy {
    use facet::Facet;
    use org_proto::BotTools;

    #[derive(Facet)]
    pub struct CreateBotRequest {
        pub session_token: String,
        pub name: String,
        pub orgs: Vec<String>,
        pub tools: BotTools,
    }
}

/// The current shape with `no_orgs` required — what 0.2.0 shipped before
/// the default. Kept so the test shows the compat check really bites.
mod required {
    use facet::Facet;
    use org_proto::BotTools;

    #[derive(Facet, Debug)]
    pub struct CreateBotRequest {
        pub session_token: String,
        pub name: String,
        pub orgs: Vec<String>,
        pub no_orgs: bool,
        pub tools: BotTools,
    }
}

fn legacy_wire() -> (vox_phon::SchemaBundle, Vec<u8>) {
    let schema = vox_phon::schema_bytes::<legacy::CreateBotRequest>().expect("legacy schema");
    let bundle = vox_phon::parse_schema_bytes(&schema).expect("parse legacy schema");
    let wire = vox_phon::to_vec(&legacy::CreateBotRequest {
        session_token: "tok".into(),
        name: "helper".into(),
        orgs: vec!["acme".into()],
        tools: BotTools::Library,
    })
    .expect("encode legacy request");
    (bundle, wire)
}

#[test]
fn a_pre_0_2_create_bot_request_decodes_with_no_orgs_false() {
    let (bundle, wire) = legacy_wire();
    let program = vox_phon::build_decode_program::<CreateBotRequest>(&bundle)
        .expect("old clients' CreateBotRequest must stay compatible");
    let decoded: CreateBotRequest =
        vox_phon::decode_owned_with_program(&program, &wire).expect("decode legacy request");
    assert_eq!(
        decoded,
        CreateBotRequest {
            session_token: "tok".into(),
            name: "helper".into(),
            orgs: vec!["acme".into()],
            no_orgs: false,
            tools: BotTools::Library,
            personal: false,
        }
    );
}

#[test]
fn without_the_default_the_old_shape_is_incompatible() {
    let (bundle, _) = legacy_wire();
    assert!(
        vox_phon::build_decode_program::<required::CreateBotRequest>(&bundle).is_err(),
        "a reader-only field with no default must be refused"
    );
}
