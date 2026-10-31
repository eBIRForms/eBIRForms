//! The MCP contract, request to response, over a scratch database: what
//! the Bot sends (provision, session open, app and assistant calls) and the
//! shapes it gets back. One test: the database handle and the key are
//! process-wide.

use std::sync::Arc;

use bir_app_core::bir_core::db::WorkerLease;
use bir_app_core::{BirApp, BranchFacts, NewTaxpayer};
use buwiz_plugin::key::PluginKey;
use buwiz_plugin::manifest::{self, Served};
use buwiz_plugin::testing::Harness;
use buwiz_plugin::{Plugin, Server, b64};
use ebirdesk_mcp::Ebirdesk;
use serde_json::{Value, json};

const TEST_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const ROOT: &str = "000123453";
const OTHER: &str = "000987654";
/// The harness's provisioned seed (`buwiz_plugin::testing`).
const HARNESS_SEED: [u8; 32] = [7; 32];

/// A whole `tools/call` result (not only its `structuredContent`).
fn call_as(bot: &mut Harness<Ebirdesk>, caller: Value, name: &str, arguments: Value) -> Value {
    let reply = bot.rpc(
        "tools/call",
        json!({ "name": name, "arguments": arguments, "_meta": { "buwiz/caller": caller } }),
    );
    reply
        .get("result")
        .cloned()
        .unwrap_or_else(|| panic!("{name}: {reply}"))
}

fn agent(bot: &mut Harness<Ebirdesk>, name: &str, arguments: Value) -> Value {
    call_as(
        bot,
        json!({ "origin": "agent", "plugin": Ebirdesk::ID, "ts": 1 }),
        name,
        arguments,
    )
}

fn result_of(answer: &Value) -> &Value {
    assert_eq!(answer["isError"], false, "{answer}");
    &answer["structuredContent"]["result"]
}

fn error_text(answer: &Value) -> String {
    assert_eq!(answer["isError"], true, "{answer}");
    answer["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn create(app: &BirApp, root: &str, name: &str) -> String {
    let taxpayer = app
        .create_taxpayer(&NewTaxpayer {
            tin_root: root.into(),
            name: name.into(),
            taxpayer_type: "Individual".into(),
            birth_date: Some("1990-01-15".into()),
            head_office: BranchFacts {
                rdo_code: "043".into(),
                registered_address: "1 Sample Street, Pasig City".into(),
                zip_code: "1600".into(),
                phone: "09171234567".into(),
                email: "juana@example.com".into(),
            },
        })
        .expect("taxpayer");
    taxpayer.branches[0].tin.clone()
}

#[test]
fn the_bot_contract_over_a_scratch_database() {
    // SAFETY: set before any thread of this test reads them (one test per process).
    unsafe {
        std::env::set_var("BUWIZ_DB_KEY", TEST_KEY);
        std::env::set_var("BUWIZ_NO_KEYCHAIN", "1");
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bir_data.db");
    let app = Arc::new(BirApp::open_at(&path).unwrap());
    let tin = create(&app, ROOT, "Juana Santos");
    let other_tin = create(&app, OTHER, "Pedro Cruz");

    // before buwiz.provision: initialize and resources answer, tools do not
    {
        let served = Served::new(None, &Ebirdesk::manifest(Ebirdesk::RELEASE_KEY_ID));
        let mut fresh = Server::with_manifest(Ebirdesk::with_app(app.clone()), served);
        let mut send = |method: &str, params: Value| -> Value {
            let line = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
                .to_string();
            serde_json::from_str(&fresh.handle_line(&line).expect("a reply")).unwrap()
        };
        let init = send("initialize", json!({ "protocolVersion": "2025-11-25" }));
        assert_eq!(init["result"]["serverInfo"]["name"], "ebirdesk-mcp");
        assert!(
            init["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("ebirdesk_profiles_list")
        );
        assert_eq!(
            send("tools/call", json!({ "name": "ebirdesk_status" }))["error"]["code"],
            -32600
        );
        assert_eq!(
            send("tools/call", json!({ "name": "buwiz.nope" }))["error"]["code"],
            -32601
        );
        let wrong = send(
            "tools/call",
            json!({ "name": "buwiz.provision", "arguments": { "plugin": "app.buwiz.ebirforms" } }),
        );
        assert_eq!(wrong["result"]["isError"], true, "{wrong}");
    }

    let mut bot = Harness::new(Ebirdesk::with_app(app.clone()));
    assert_eq!(bot.rpc("nope/nope", json!({}))["error"]["code"], -32601);

    // tools and resources
    let tools = bot.rpc("tools/list", json!({}))["result"]["tools"]
        .as_array()
        .unwrap()
        .clone();
    let tool = |name: &str| {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("no {name}"))
    };
    for name in [
        "ebirdesk_status",
        "ebirdesk_profiles_list",
        "ebirdesk_form_open",
        "ebirdesk_form_send",
        "ebirdesk_app_open",
        "ebirdesk_resolve",
    ] {
        assert!(
            !tool(name)["description"]
                .as_str()
                .unwrap()
                .starts_with("Mini apps only"),
            "{name} is the assistant's"
        );
    }
    for name in [
        "ebirdesk_app_screen_form_open",
        "ebirdesk_app_screen_form_save",
        "ebirdesk_ticket_subject",
        "ebirdesk_app_print_preview",
    ] {
        assert!(
            tool(name)["description"]
                .as_str()
                .unwrap()
                .starts_with("Mini apps only"),
            "{name}"
        );
    }
    assert_eq!(
        tool("ebirdesk_app_screen_form_validate")["annotations"]["readOnlyHint"],
        true
    );
    assert_eq!(
        tool("ebirdesk_app_screen_form_save")["annotations"]["readOnlyHint"],
        false
    );
    let resources = bot.rpc("resources/list", json!({}))["result"]["resources"].clone();
    let uris: Vec<&str> = resources
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["uri"].as_str().unwrap())
        .collect();
    assert_eq!(uris, ["buwiz://manifest", "buwiz-app://forms"]);
    let manifest: Value =
        serde_json::from_str(
            bot.rpc("resources/read", json!({ "uri": "buwiz://manifest" }))["result"]["contents"]
                [0]["text"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(manifest["id"], "app.buwiz.ebirdesk");
    let forms =
        bot.rpc("resources/read", json!({ "uri": "buwiz-app://forms" }))["result"]["contents"][0]
            .clone();
    assert_eq!(forms["mimeType"], "text/x-nsh-tsx");
    let source = forms["text"].as_str().unwrap();
    assert!(source.contains("ebirdesk_app_") && source.contains(r#"from "buwiz""#));
    assert_eq!(
        manifest["apps"][0]["fingerprint"],
        manifest::fingerprint(source)
    );
    assert!(bot.rpc("resources/read", json!({ "uri": "file:///etc/passwd" }))["error"].is_object());
    // every function the Forms app may call exists in bir-app-core
    let names = app.function_names();
    for tool in manifest["apps"][0]["permissions"]["tools"]
        .as_array()
        .unwrap()
    {
        if let Some(function) = tool
            .as_str()
            .unwrap()
            .strip_prefix("ebirdesk_app_")
            .filter(|f| *f != "print_preview")
        {
            assert!(
                names.contains(&function),
                "{function} is not a BirApp function"
            );
        }
    }

    // the assistant: taxpayers as tickets, never TINs
    let status = agent(&mut bot, "ebirdesk_status", json!({}));
    let status = result_of(&status).clone();
    assert_eq!(status["taxpayers"], 2, "{status}");
    assert_eq!(status["app"], "ebirdesk-mcp");
    let listed = agent(&mut bot, "ebirdesk_profiles_list", json!({}));
    let text = listed["content"][0]["text"].as_str().unwrap();
    assert!(
        !text.contains(ROOT) && !text.contains(OTHER),
        "a TIN reached the model: {text}"
    );
    let juana = result_of(&listed)
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "Juana Santos")
        .unwrap()
        .clone();
    let ticket = juana["tin_root"].as_str().unwrap().to_string();
    assert!(ticket.starts_with("tkt_"), "{juana}");
    let branch_ticket = juana["branches"][0]["tin"].as_str().unwrap().to_string();
    assert_eq!(branch_ticket, ticket, "the head office is the taxpayer");

    // the Bot resolves a ticket's title and the taxprofile scope
    let title = call_as(
        &mut bot,
        json!({}),
        "buwiz.ticket.resolve",
        json!({ "ticket": ticket }),
    );
    assert_eq!(
        title["structuredContent"],
        json!({ "valid": true, "title": "Juana Santos" })
    );
    let resolved = bot.resolve(
        "ebirdesk_resolve",
        json!({ "kind": "scope", "name": "taxprofile", "value": "juana" }),
    );
    assert_eq!(
        resolved,
        Ok(json!({ "result": "token", "ticket": ticket, "title": "Juana Santos" }))
    );
    let choices = bot
        .resolve(
            "ebirdesk_resolve",
            json!({ "kind": "scope", "name": "taxprofile", "value": "", "complete": true }),
        )
        .unwrap();
    assert_eq!(choices["choices"].as_array().unwrap().len(), 2, "{choices}");
    // a resolve call never writes
    let refused = bot
        .resolve("ebirdesk_form_save_draft", json!({}))
        .unwrap_err();
    assert_eq!(refused["code"], "permission_denied");

    // the assistant cannot call the app's tools
    let refused = agent(
        &mut bot,
        "ebirdesk_app_list_tax_years",
        json!({ "args": [tin], "ticket": ticket }),
    );
    assert_eq!(
        refused["structuredContent"]["code"], "permission_denied",
        "{refused}"
    );

    // ebirdesk_app_open hands the Bot the Forms app on a return's ticket
    let opened = agent(
        &mut bot,
        "ebirdesk_app_open",
        json!({ "app": "forms", "tin": ticket, "form": "2551Q", "period": "2026-Q3", "action": "file" }),
    );
    let open = &opened["_meta"]["buwiz/open"];
    assert_eq!(open["app"], "forms", "{opened}");
    assert_eq!(open["placement"], "full");
    assert_eq!(open["props"]["form"], "2551Q");
    assert_eq!(open["props"]["period"], "2026-Q3");
    let return_ticket = open["props"]["ticket"].as_str().unwrap().to_string();
    assert!(return_ticket.starts_with("tkt_") && return_ticket != ticket);
    assert!(!opened.to_string().contains(ROOT));

    // an app open, certified with the provisioned plugin key
    let entry = manifest::app(&manifest, "forms").unwrap().clone();
    let open_id = b64::encode(&[1u8; 16]);
    let not_after = chrono::Utc::now().timestamp_millis() + 3_600_000;
    let session = call_as(
        &mut bot,
        json!({}),
        "buwiz.session.open",
        json!({
            "app": "forms", "open_id": open_id, "ks_pub": b64::encode(&[2u8; 32]),
            "perms_sha256": manifest::permissions_digest(&entry).unwrap(), "not_after": not_after,
        }),
    );
    let cert = session["structuredContent"]["cert"].clone();
    assert_eq!(cert["plugin"], "app.buwiz.ebirdesk", "{session}");
    assert_eq!(cert["typ"], "buwiz-session");
    assert_eq!(
        session["structuredContent"]["cert_sig"],
        PluginKey::from_seed(&HARNESS_SEED).sign(&cert).unwrap()
    );
    let forms_app = json!({ "origin": "app", "plugin": Ebirdesk::ID, "app": "forms", "open_id": open_id, "ts": 1 });
    let stranger = json!({ "origin": "app", "plugin": Ebirdesk::ID, "app": "forms", "open_id": b64::encode(&[9u8; 16]), "ts": 1 });
    let refused = call_as(
        &mut bot,
        stranger,
        "ebirdesk_ticket_subject",
        json!({ "ticket": ticket }),
    );
    assert_eq!(refused["structuredContent"]["code"], "permission_denied");
    // an app calls only what its manifest entry lists
    let refused = call_as(
        &mut bot,
        forms_app.clone(),
        "ebirdesk_form_save_draft",
        json!({}),
    );
    assert_eq!(
        refused["structuredContent"]["code"], "permission_denied",
        "{refused}"
    );

    // the app learns its ticket's subject, and is held to it
    let subject = call_as(
        &mut bot,
        forms_app.clone(),
        "ebirdesk_ticket_subject",
        json!({ "ticket": return_ticket }),
    );
    let subject = result_of(&subject).clone();
    assert_eq!(subject["kind"], "return");
    assert_eq!(subject["tin"], tin);
    assert_eq!(subject["form"], "2551Q");
    let years = call_as(
        &mut bot,
        forms_app.clone(),
        "ebirdesk_app_list_tax_years",
        json!({ "args": [tin], "ticket": ticket }),
    );
    assert_eq!(years["isError"], false, "{years}");
    let other = call_as(
        &mut bot,
        forms_app.clone(),
        "ebirdesk_app_list_tax_years",
        json!({ "args": [other_tin], "ticket": ticket }),
    );
    assert!(
        error_text(&other).contains("outside this app's scope"),
        "{other}"
    );
    let no_ticket = call_as(
        &mut bot,
        forms_app.clone(),
        "ebirdesk_app_list_tax_years",
        json!({ "args": [tin] }),
    );
    assert!(error_text(&no_ticket).contains("ticket"));
    let rows = call_as(
        &mut bot,
        forms_app.clone(),
        "ebirdesk_app_list_taxpayers",
        json!({ "ticket": ticket }),
    );
    let rows = result_of(&rows).as_array().unwrap().clone();
    assert_eq!(rows.len(), 1, "a list is held to the ticket's taxpayer");
    assert_eq!(rows[0]["ticket"], ticket);
    let payments = call_as(
        &mut bot,
        forms_app.clone(),
        "ebirdesk_payments_list",
        json!({ "tin": ticket, "form": "2551Q", "period": "2026-Q3" }),
    );
    assert_eq!(result_of(&payments), &json!({ "payments": [] }));
    let opened = call_as(
        &mut bot,
        forms_app.clone(),
        "ebirdesk_app_screen_form_open",
        json!({ "args_json": json!([tin, "2551Q", 2026, 3, null]).to_string(), "ticket": ticket }),
    );
    let draft = result_of(&opened)["draft"].clone();
    assert!(draft.is_object(), "{opened}");

    // the assistant drafts a return
    let form = agent(
        &mut bot,
        "ebirdesk_form_open",
        json!({ "code": "2551Q", "tin": ticket, "period": "2026-Q3" }),
    );
    let summary = result_of(&form).clone();
    assert_eq!(summary["form"], "2551Q", "{form}");
    assert_eq!(
        summary["tin"], ticket,
        "the open return names its taxpayer by ticket"
    );
    assert_eq!(summary["year"], 2026);
    assert_eq!(summary["period"], 3);
    let fields = agent(&mut bot, "ebirdesk_form_fields", json!({}));
    assert!(
        result_of(&fields)["values"]
            .as_object()
            .is_some_and(|values| !values.is_empty()),
        "{fields}"
    );
    let checked = agent(&mut bot, "ebirdesk_form_validate", json!({}));
    assert!(result_of(&checked)["field_errors"].is_array(), "{checked}");
    assert!(
        error_text(&agent(
            &mut bot,
            "ebirdesk_form_fill",
            json!({ "fields": {} })
        ))
        .contains("fields")
    );
    let filled = agent(
        &mut bot,
        "ebirdesk_form_fill",
        json!({ "fields": { "no-such-box": "1.00" }, "source": "document" }),
    );
    assert_eq!(filled["isError"], false, "{filled}");
    // unsaved edits keep it open
    assert!(error_text(&agent(&mut bot, "ebirdesk_form_dismiss", json!({}))).contains("unsaved"));

    // the desktop app open beside this process (it holds the worker lease):
    // the database is shared, so saving and granting go through here
    let desktop_worker = WorkerLease::try_acquire(&path)
        .unwrap()
        .expect("the desktop app's worker lease");
    let saved = agent(&mut bot, "ebirdesk_form_save_draft", json!({}));
    let saved = result_of(&saved).clone();
    assert_eq!(saved["saved"], true, "{saved}");
    let draft_id = saved["id"].as_i64().expect("a saved id");
    let drafts = agent(
        &mut bot,
        "ebirdesk_drafts_list",
        json!({ "tin": ticket, "status": "all" }),
    );
    let drafts = result_of(&drafts).as_array().unwrap().clone();
    assert!(
        drafts
            .iter()
            .any(|row| row["draft_id"] == draft_id && row["form"] == "2551Q"),
        "{drafts:?}"
    );
    assert!(drafts.iter().all(|row| row["tin"] == ticket));

    // sending needs the user's standing OK
    let send = agent(&mut bot, "ebirdesk_form_send", json!({}));
    assert_eq!(result_of(&send)["status"], "approval_required", "{send}");
    let granted = call_as(
        &mut bot,
        forms_app.clone(),
        "ebirdesk_app_grant_send",
        json!({ "args": [tin, "2551Q"], "ticket": ticket }),
    );
    assert_eq!(result_of(&granted)["form"], "2551Q", "{granted}");
    let grants = call_as(
        &mut bot,
        forms_app.clone(),
        "ebirdesk_app_list_send_grants",
        json!({ "args": [tin], "ticket": ticket }),
    );
    assert_eq!(result_of(&grants).as_array().unwrap().len(), 1);
    // this process never took the desktop app's lease
    drop(desktop_worker);
    assert!(
        WorkerLease::try_acquire(&path).unwrap().is_some(),
        "no worker lease left behind"
    );

    // the print preview of the saved return, for the Bot's document view
    let preview = call_as(
        &mut bot,
        forms_app.clone(),
        "ebirdesk_app_print_preview",
        json!({ "ticket": ticket, "form": "2551Q", "draft_id": draft_id }),
    );
    assert_eq!(preview["isError"], false, "{preview}");
    assert_eq!(preview["content"][0]["resource"]["mimeType"], "text/html");
    assert!(
        preview["content"][0]["resource"]["text"]
            .as_str()
            .unwrap()
            .contains("<")
    );

    // dismissed after saving; buwiz.uninstall forgets every ticket
    assert_eq!(
        result_of(&agent(&mut bot, "ebirdesk_form_dismiss", json!({})))["dismissed"],
        true
    );
    assert_eq!(
        call_as(&mut bot, json!({}), "buwiz.uninstall", json!({}))["structuredContent"]["ok"],
        true
    );
    let gone = call_as(
        &mut bot,
        json!({}),
        "buwiz.ticket.resolve",
        json!({ "ticket": ticket }),
    );
    assert_eq!(gone["structuredContent"]["valid"], false);
    bot.plugin_mut().finish();
}
