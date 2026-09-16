#![no_main]

use libfuzzer_sys::fuzz_target;

// Fuzzes the `.cnxp` artifact header parser (`parse_artifact_header`) —
// the header is attacker-controlled input on the `import` path (team
// artifacts come from third parties). Invariants: the parser never panics;
// a successful parse implies a header that is a strict prefix of the input
// and carries the current format version.
fuzz_target!(|data: &[u8]| {
    if let Ok((manifest, header_total)) = codenexus::service::import::parse_artifact_header(data) {
        assert!(
            header_total >= 8 && header_total <= data.len(),
            "header size {header_total} out of bounds for {} input bytes",
            data.len()
        );
        assert_eq!(
            manifest.format_version,
            codenexus::service::export::ARTIFACT_FORMAT_VERSION,
            "accepted artifact must carry the current format version"
        );
    }
});
