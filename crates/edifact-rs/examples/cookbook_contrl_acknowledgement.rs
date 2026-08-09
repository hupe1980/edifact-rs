//! Answer an inbound interchange with a `CONTRL` acknowledgement (ISO 9735-4).
//!
//! The whole point of building the acknowledgement from the validation report is
//! that the two cannot disagree: what you send back is derived from what you
//! actually found, not assembled by hand alongside it.
//!
//! Run with:
//!
//! ```sh
//! cargo run --example cookbook_contrl_acknowledgement
//! ```

use edifact_rs::{Contrl, EdifactError, ValidationContext, from_bytes, validate_envelope_lenient};

/// A clean interchange: nothing to report but its arrival and acceptance.
const CLEAN: &[u8] = b"UNB+UNOC:3+SENDER:14+RECEIVER:14+260101:0900+IC4711++++1'\
                       UNH+MSG1+ORDERS:D:96A:UN'\
                       BGM+220+PO-4711+9'\
                       DTM+137:20260101:102'\
                       UNT+4+MSG1'\
                       UNZ+1+IC4711'";

/// The same interchange with two faults: a value of nothing but spaces
/// (ISO 9735-1 §9.3) and a `UNT` count that does not match reality.
const FAULTY: &[u8] = b"UNB+UNOC:3+SENDER:14+RECEIVER:14+260101:0900+IC4712'\
                        UNH+MSG1+ORDERS:D:96A:UN'\
                        BGM+220+PO-4712+9'\
                        FTX+   '\
                        UNT+9+MSG1'\
                        UNZ+1+IC4712'";

fn main() -> Result<(), EdifactError> {
    println!("── a clean interchange ──────────────────────────────────────────");
    let segments: Vec<_> = from_bytes(CLEAN).collect::<Result<Vec<_>, _>>()?;
    let validated = validate_envelope_lenient(&segments)
        .interchange
        .expect("structurally interpretable");

    // DE 0031 asked for an acknowledgement, so one is owed.
    println!(
        "acknowledgement requested: {}",
        validated.interchange.ack_requested()
    );

    // The receipt, sent immediately — action code 8, nothing checked yet.
    let receipt = Contrl::receipt(&validated.interchange).with_message_reference("RCPT1");
    println!("receipt:         {}", receipt.to_edifact_string()?);

    // The acknowledgement, sent after the syntax check — action code 7.
    let ack = Contrl::acknowledgement(&validated).with_message_reference("ACK1");
    println!("acknowledgement: {}", ack.to_edifact_string()?);

    // Ready to send: its own interchange, addressed back the way it came.
    println!(
        "on the wire:     {}",
        ack.to_interchange_string("UNOC", "3", "260101", "0930", "ACK-IC-1")?
    );

    println!();
    println!("── an interchange with faults ───────────────────────────────────");
    let segments: Vec<_> = from_bytes(FAULTY).collect::<Result<Vec<_>, _>>()?;

    // The lenient path is the one that yields both an interchange and its
    // faults, which is exactly what a CONTRL reports.
    let validated = validate_envelope_lenient(&segments)
        .interchange
        .expect("structurally interpretable");
    let report = ValidationContext::builder()
        .with_envelope_validation()
        .with_syntax_validation()
        .build()
        .validate_lenient(&segments);

    for issue in report.errors().iter().chain(report.warnings()) {
        println!(
            "  {} {:?} — {}",
            issue.error_code().unwrap_or("—"),
            issue.severity,
            issue.message
        );
    }

    let contrl = Contrl::from_report(&validated, &segments, &report).with_message_reference("NAK1");
    println!(
        "verdict:         {:?} (DE 0083 = {})",
        contrl.action(),
        contrl.action()
    );
    println!("report:          {}", contrl.to_edifact_string()?);

    // Note what the verdict is *not*: the interchange envelope itself is sound,
    // so the UCI stays at 7.  Code 4 there would mean "this level and all lower
    // levels rejected" (ISO 9735-4 §5.3.2) and would take down every other
    // message in the interchange along with the one that is actually broken.
    //
    // Each finding then sits at the lowest reporting level that can both locate
    // it and legally carry its code:
    //   UCM+…+4+29 — the message is rejected; code 29 may not go any lower
    //   UCS+n      — the n-th segment of the message, UNH being 1
    //   UCD+code+p — data element p of that segment, the tag being position 1
    Ok(())
}
