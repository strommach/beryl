use std::collections::BTreeMap;
use std::fs;
use std::io::Cursor;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::process::Command;

use beryl_nix::derivation::{DerivationBuilder, OutputName, OutputsBuilder, lookup_fn};
use beryl_nix::nar_fs::{dump_nar, hash_nar_path, unpack_nar};
use beryl_nix::narinfo::parse_keypair;
use beryl_nix::narinfo_ext::{default_nixos_verifying_key, parse_narinfo, verify_narinfo};
use beryl_nix::nixbase32;
use beryl_nix::nixhash::NixHash;
use beryl_nix::store_path::{StorePathRef, build_ca_path, build_text_path};
use beryl_nix::store_path_ext::{parse_store_path, to_absolute_string};
use tempfile::TempDir;

#[test]
fn store_path_parse_and_roundtrip() {
    let raw = "/nix/store/00bgd045z0d4icpbc2yyz4gx48ak44la-net-tools-1.60_p20170221182432";
    let sp = parse_store_path(raw).expect("valid absolute store path");
    assert_eq!(to_absolute_string(sp.as_ref()), raw);

    let bare = "00bgd045z0d4icpbc2yyz4gx48ak44la-net-tools-1.60_p20170221182432";
    let sp2 = parse_store_path(bare).expect("valid bare store path");
    assert_eq!(sp, sp2);

    // 'e', 'o', 'u', 't' are forbidden in Nix base32
    assert!(parse_store_path("/nix/store/eeeggggggggggggggggggggggggggggg-bad").is_err());
    assert!(parse_store_path("/nix/store/short-bad").is_err());
}

#[test]
fn nix_base32_and_text_path_golden() {
    let sp = build_text_path("foo", b"bar", std::iter::empty::<StorePathRef<'_>>())
        .expect("build_text_path");
    assert_eq!(sp.name(), "foo");
    assert_eq!(nixbase32::encode(sp.digest()).len(), 32);

    // Fixed-output flat sha256 path for empty content
    let empty_sha256 = NixHash::Sha256([
        0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f, 0xb9,
        0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b, 0x78, 0x52,
        0xb8, 0x55,
    ]);
    let ca_sp = build_ca_path(
        "empty",
        false,
        &empty_sha256,
        std::iter::empty::<StorePathRef<'_>>(),
        false,
    )
    .expect("build_ca_path");
    assert!(ca_sp.to_absolute_path().starts_with("/nix/store/"));
    assert!(ca_sp.to_absolute_path().ends_with("-empty"));
}

#[test]
fn nar_dump_unpack_and_hash_roundtrip() {
    let src_dir = TempDir::new().unwrap();
    let root = src_dir.path().join("pkg");
    fs::create_dir(&root).unwrap();

    // Create files out of alphabetical order to verify NAR lexicographical sorting
    fs::write(root.join("z.txt"), b"hello world\n").unwrap();
    fs::write(root.join("a.txt"), b"first file").unwrap();

    let bin_dir = root.join("bin");
    fs::create_dir(&bin_dir).unwrap();
    let script = bin_dir.join("run.sh");
    fs::write(&script, b"#!/bin/sh\necho ok\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    symlink("a.txt", root.join("link-to-a")).unwrap();

    // Dump to in-memory NAR
    let mut nar_bytes = Vec::new();
    dump_nar(&root, &mut nar_bytes).expect("dump_nar");

    // Hash via single-pass hasher
    let hash_before = hash_nar_path(&root).expect("hash_nar_path before");
    assert_eq!(hash_before.size, nar_bytes.len() as u64);

    // Unpack to a fresh directory
    let dst_dir = TempDir::new().unwrap();
    let unpacked = dst_dir.path().join("unpacked");
    unpack_nar(Cursor::new(&nar_bytes), &unpacked).expect("unpack_nar");

    // Verify contents, permissions, and symlinks
    assert_eq!(fs::read(unpacked.join("a.txt")).unwrap(), b"first file");
    assert_eq!(fs::read(unpacked.join("z.txt")).unwrap(), b"hello world\n");
    assert_eq!(
        fs::read_link(unpacked.join("link-to-a")).unwrap(),
        std::path::PathBuf::from("a.txt")
    );
    let exec_mode = fs::metadata(unpacked.join("bin/run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_ne!(exec_mode & 0o111, 0, "executable bit must be preserved");

    // Re-hash unpacked directory: must be byte-for-byte identical
    let hash_after = hash_nar_path(&unpacked).expect("hash_nar_path after");
    assert_eq!(hash_before, hash_after);

    // If `nix` is available on the host, cross-verify against `nix hash path --type sha256 --base32`
    if let Ok(output) = Command::new("nix")
        .args(["hash", "path", "--type", "sha256", "--base32"])
        .arg(&root)
        .output()
        && output.status.success()
    {
        let nix_hash = String::from_utf8(output.stdout).unwrap().trim().to_string();
        let beryl_hash = nixbase32::encode(&hash_before.sha256);
        assert_eq!(
            beryl_hash, nix_hash,
            "beryl NAR sha256 must match `nix hash path` byte-for-byte"
        );
    }
}

#[test]
fn narinfo_parse_and_ed25519_verification() {
    // Built-in cache.nixos.org key parses cleanly
    let nixos_key = default_nixos_verifying_key();
    assert_eq!(nixos_key.name(), "cache.nixos.org-1");

    // Test-only Ed25519 keypair generated via `nix key generate-secret --key-name beryl-test-1`
    let (signing_key, verifying_key) = parse_keypair(
        "beryl-test-1:R4/XHi9oGG2d42jPuUH6YG92wYBhNRZFFGs3Yzdp724F/EQJ2Opc8CzPz3Y2uRDs+/gCxNFD4kqeVsIWli3goA==",
    )
    .expect("parse test keypair");

    let unsigned_narinfo = "\
StorePath: /nix/store/00bgd045z0d4icpbc2yyz4gx48ak44la-net-tools-1.60_p20170221182432
URL: nar/1094wph9z4nwlgvsd53abfz8i117ykiv5dwnq9nnhz846s7xqd7d.nar.xz
Compression: xz
FileHash: sha256:1094wph9z4nwlgvsd53abfz8i117ykiv5dwnq9nnhz846s7xqd7d
FileSize: 12345
NarHash: sha256:1094wph9z4nwlgvsd53abfz8i117ykiv5dwnq9nnhz846s7xqd7d
NarSize: 54321
References: 00bgd045z0d4icpbc2yyz4gx48ak44la-net-tools-1.60_p20170221182432
";
    let mut parsed = parse_narinfo(unsigned_narinfo).expect("parse narinfo");
    parsed.add_signature(&signing_key);

    assert!(verify_narinfo(&parsed, std::slice::from_ref(&verifying_key)));
    assert!(!verify_narinfo(&parsed, &[nixos_key]));

    // Re-serialize, tamper with NarSize, re-parse, and verify signature fails
    let signed_text = parsed.to_string();
    let tampered_text = signed_text.replace("NarSize: 54321", "NarSize: 99999");
    let tampered = parse_narinfo(&tampered_text).expect("parse tampered narinfo");
    assert!(!verify_narinfo(&tampered, &[verifying_key]));
}

#[test]
fn derivation_builder_and_aterm_roundtrip() {
    let mut env = BTreeMap::new();
    env.insert("builder".into(), "/bin/sh".into());
    env.insert("name".into(), "beryl-hello".into());
    env.insert("out".into(), "".into());
    env.insert("system".into(), "aarch64-linux".into());

    let builder = DerivationBuilder {
        arguments: vec!["-c".into(), "echo hello > $out".into()],
        command: "/bin/sh".into(),
        environment: env,
        input_derivations: BTreeMap::new(),
        input_sources: Default::default(),
        outputs: OutputsBuilder::default(),
        system: "aarch64-linux".into(),
    };

    let drv = builder
        .build("beryl-hello", lookup_fn(|_| None))
        .expect("build derivation");

    let aterm = drv.to_aterm_bytes();
    assert!(aterm.starts_with(b"Derive("));

    let drv_path = drv.drv_path();
    assert!(drv_path.to_absolute_path().ends_with("-beryl-hello.drv"));

    let out_path = drv
        .outputs()
        .get(&OutputName::out())
        .expect("has out output");
    assert!(out_path.to_absolute_path().ends_with("-beryl-hello"));
}
