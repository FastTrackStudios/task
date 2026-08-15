//! End-to-end smoke — exercise fulgur all the way through to
//! PDF bytes and verify the magic number. Doesn't assert on
//! page count / layout / fonts; those are fulgur's concern.

use pdf::{render_html, render_invoice, InvoiceData, InvoiceLine, InvoiceParty};

fn looks_like_pdf(b: &[u8]) -> bool {
    b.starts_with(b"%PDF-")
}

#[test]
fn render_hello_html() {
    let bytes = render_html("<h1>Hello, PDF.</h1>").expect("render");
    assert!(looks_like_pdf(&bytes), "first 8 bytes: {:?}", &bytes[..8.min(bytes.len())]);
    assert!(bytes.len() > 200, "tiny output — likely empty page");
}

#[test]
fn render_minimal_invoice() {
    let data = InvoiceData {
        number: "INV-2026-0001".into(),
        currency: "USD".into(),
        currency_symbol: "$".into(),
        issue_date: "2026-05-22".into(),
        due_date: "2026-06-21".into(),
        status: "sent".into(),
        from: InvoiceParty {
            name: "Cody Wright".into(),
            address: "Houston, TX".into(),
            email: "cody@example.com".into(),
            phone: String::new(),
            tax_id: String::new(),
        },
        to: InvoiceParty {
            name: "ACME Corp".into(),
            address: "123 Market St\nAustin, TX 78701".into(),
            email: "billing@acme.test".into(),
            phone: String::new(),
            tax_id: String::new(),
        },
        lines: vec![
            InvoiceLine {
                description: "Navbar bug fix + responsive review".into(),
                quantity: "4.50".into(),
                unit: "hr".into(),
                unit_price: "150.00".into(),
                amount: "675.00".into(),
            },
            InvoiceLine {
                description: "Sprint planning + standup support".into(),
                quantity: "1.25".into(),
                unit: "hr".into(),
                unit_price: "150.00".into(),
                amount: "187.50".into(),
            },
        ],
        subtotal: "862.50".into(),
        tax: String::new(),
        total: "862.50".into(),
        balance_due: "862.50".into(),
        notes: "Net 30. Wire details on request.".into(),
        terms: String::new(),
        footer: "Thanks for the work.".into(),
    };
    let bytes = render_invoice(&data).expect("render");
    assert!(looks_like_pdf(&bytes));
    assert!(
        bytes.len() > 5_000,
        "invoice render produced suspiciously small output: {}",
        bytes.len(),
    );
}
