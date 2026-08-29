use edifact_rs::{Element, Segment};

/// One representative D.11A message (~230 bytes).
pub fn sample_msg() -> &'static [u8] {
    b"\
UNA:+.? '\
UNB+UNOA:3+9900000000001::293+9900000000002::293+260401:0900+1'\
UNH+1+ORDERS:D:11A:UN'\
BGM+220+PO-4711+9'\
DTM+137:20260401:102'\
NAD+BY+4000001000002::9'\
NAD+SU+4000001000001::9'\
LIN+1++4012345678901:SRV'\
QTY+21:10:PCE'\
PRI+AAA:12.50'\
UNT+8+1'\
UNZ+1+1'"
}

/// ~1 MB of repeated messages — a **byte-level** fixture.
///
/// `sample_msg()` begins with a 9-byte UNA header (`UNA:+.? '`).  Only the
/// first copy carries it; later copies omit it, because a mid-stream `UNA:` is
/// an ordinary (and invalid) segment tag rather than a second service string
/// advice.
///
/// The result is a stream of *repeated interchanges*, which is the right shape
/// for the tokenizer, parser, reader, and writer benchmarks — they measure
/// bytes in, segments out, and never look at the envelope.  It is the wrong
/// shape for anything that validates: `UNB` appears again inside the first
/// interchange, so envelope extraction aborts a few segments in and the
/// benchmark stops measuring almost immediately.  Use
/// [`one_mb_interchange`] there.
pub fn one_mb() -> &'static [u8] {
    static ONE_MB: std::sync::OnceLock<&'static [u8]> = std::sync::OnceLock::new();
    ONE_MB.get_or_init(|| {
        const UNA_LEN: usize = 9; // b"UNA:+.? '" is exactly 9 bytes
        let msg = sample_msg();
        let inner = &msg[UNA_LEN..]; // message body without the UNA header
        let reps = (1_000_000 / msg.len()) + 1;
        let mut data = Vec::with_capacity(reps * msg.len());
        data.extend_from_slice(msg); // first copy: includes the UNA
        for _ in 1..reps {
            data.extend_from_slice(inner); // subsequent copies: no UNA
        }
        // Intentional leak: stored in OnceLock so it leaks at most once per process.
        // Avoids lifetime complexity in benchmark setup; safe for bench-only code.
        Box::leak(data.into_boxed_slice())
    })
}

pub fn sample_segments() -> Vec<Segment<'static>> {
    vec![
        Segment::new(
            "UNB",
            vec![
                Element::of(&["UNOA", "3"]),
                Element::of(&["9900000000001", "", "293"]),
                Element::of(&["9900000000002", "", "293"]),
                Element::of(&["260401", "0900"]),
                Element::of(&["1"]),
            ],
        ),
        Segment::new(
            "UNH",
            vec![
                Element::of(&["1"]),
                Element::of(&["ORDERS", "D", "11A", "UN"]),
            ],
        ),
        Segment::new(
            "BGM",
            vec![
                Element::of(&["220"]),
                Element::of(&["PO-4711"]),
                Element::of(&["9"]),
            ],
        ),
        Segment::new("DTM", vec![Element::of(&["137", "20260401", "102"])]),
        Segment::new(
            "NAD",
            vec![
                Element::of(&["BY"]),
                Element::of(&["4000001000002", "", "9"]),
            ],
        ),
        Segment::new("UNT", vec![Element::of(&["5"]), Element::of(&["1"])]),
        Segment::new("UNZ", vec![Element::of(&["1"]), Element::of(&["1"])]),
    ]
}

/// ~1 MB as a **single conformant interchange**: one `UNB`, many `UNH`/`UNT`
/// messages, one `UNZ` whose DE 0036 matches what was written.
///
/// This is the fixture for anything that validates. Its predecessor was a
/// stream of repeated interchanges, on which `validate_envelope` aborts at the
/// second `UNB` — so a "validate 1 MB" benchmark measured the first handful of
/// segments and reported a throughput figure for work it never did.
pub fn one_mb_interchange() -> &'static [u8] {
    static ONE_MB: std::sync::OnceLock<&'static [u8]> = std::sync::OnceLock::new();
    ONE_MB.get_or_init(|| {
        // One message: UNH + 7 body segments + UNT.
        const BODY: &[u8] = b"\
BGM+220+PO-4711+9'\
DTM+137:20260401:102'\
NAD+BY+4000001000002::9'\
NAD+SU+4000001000001::9'\
LIN+1++4012345678901:SRV'\
QTY+21:10:PCE'\
PRI+AAA:12.50'";
        const BODY_SEGMENTS: usize = 7;

        let header = b"UNA:+.? 'UNB+UNOA:3+9900000000001::293+9900000000002::293+260401:0900+IC1'";
        // Each message costs its UNH, the body, and its UNT.
        let per_message = 24 + BODY.len() + 10;
        let messages = 1_000_000 / per_message + 1;

        let mut data = Vec::with_capacity(messages * per_message + header.len() + 32);
        data.extend_from_slice(header);
        for reference in 1..=messages {
            data.extend_from_slice(format!("UNH+{reference}+ORDERS:D:11A:UN'").as_bytes());
            data.extend_from_slice(BODY);
            // UNT DE 0074 counts UNH + body + UNT.
            data.extend_from_slice(format!("UNT+{}+{reference}'", BODY_SEGMENTS + 2).as_bytes());
        }
        data.extend_from_slice(format!("UNZ+{messages}+IC1'").as_bytes());

        // Intentional leak: stored in a OnceLock, so at most once per process.
        Box::leak(data.into_boxed_slice())
    })
}
