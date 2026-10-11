//! Regenerates the ImageJ reference fixtures in `fixtures/imagej/out`.
//!
//!   cargo xtask gen       run every fixtures/imagej/gen/*.ijm with real ImageJ 1.x and
//!                         rewrite fixtures/imagej/out and its manifest.toml
//!   cargo xtask verify    regenerate into target/verify-out and compare with the manifest
//!
//! Java and ij.jar are needed only to run this tool. The generated files are committed, so
//! neither is a build, test or runtime dependency of ImageR2.
//!
//! ImageJ cannot run these macros with `-Djava.awt.headless=true`; it is run with `-batch`,
//! which creates no main window but needs a display. See docs/spikes/s4-s5-fixtures-tiff.md.
//!
//! Environment overrides:
//!   IJ_JAR             path to ij.jar (default: ../reference/ij/ij-1.54p.jar)
//!   IJ_ALLOW_UNPINNED  "1" accepts a jar whose hash differs from the pinned one
//!   IJ_JAVA            java executable (default: java on PATH)
//!   IJ_HEADLESS        "1" adds -Djava.awt.headless=true (does not work for run(...))
//!   IJ_TIMEOUT         per-macro timeout in seconds (default 120; a macro error opens a
//!                      modal dialog and would otherwise hang)

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use std::{env, fs, io};

/// SHA-256 of net.imagej:ij:1.54p from Maven Central (SHA-1 5a677e50910599433e65f4b5bb390e5cd1f794dd).
const PINNED_JAR_SHA256: &str = "2e1a09961dfb41cee66ddc821b2577a41a072566ce45a49bae69267099741e20";
/// Outputs known not to be byte-reproducible (zip entry timestamps).
const VOLATILE: &[&str] = &["roiset.zip"];
const SENTINEL: &str = "GEN_OK";

fn main() {
    let cmd = env::args().nth(1).unwrap_or_else(|| "gen".into());
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let res = match cmd.as_str() {
        "gen" => generate(&root, &root.join("fixtures/imagej/out")).map(|_| ()),
        "verify" => verify(&root),
        other => Err(format!(
            "unknown subcommand {other:?} (expected gen | verify)"
        )),
    };
    if let Err(e) = res {
        eprintln!("xtask: error: {e}");
        std::process::exit(1);
    }
}

struct Entry {
    sha256: String,
    bytes: u64,
    macro_name: String,
}

struct Run {
    files: BTreeMap<String, Entry>,
    ij_version: String,
    java_version: String,
    jar_sha256: String,
    jar_name: String,
    headless: bool,
}

fn fwd(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    s.strip_prefix("//?/").map(str::to_owned).unwrap_or(s)
}

fn sha256_file(p: &Path) -> io::Result<String> {
    let data = fs::read(p)?;
    let mut s = String::new();
    for b in Sha256::digest(&data) {
        write!(s, "{b:02x}").unwrap();
    }
    Ok(s)
}

fn snapshot(dir: &Path) -> io::Result<BTreeMap<String, (String, u64)>> {
    let mut m = BTreeMap::new();
    for e in fs::read_dir(dir)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        if e.file_type()?.is_file() && name != "manifest.toml" {
            m.insert(name, (sha256_file(&e.path())?, e.metadata()?.len()));
        }
    }
    Ok(m)
}

/// Runs one macro file in its own JVM. Returns (stdout, stderr).
fn run_macro(
    root: &Path,
    jar: &Path,
    macro_path: &Path,
    arg: &str,
    headless: bool,
) -> Result<(String, String), String> {
    // An empty cwd + user.home keeps ImageJ from loading any IJ_Prefs.txt (user prefs change e.g. TIFF byte order).
    let home = root.join("target").join("ij-clean-home");
    fs::create_dir_all(&home).map_err(|e| e.to_string())?;
    let logs = root.join("target").join("gen-logs");
    fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
    let stem = macro_path
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let out_log = logs.join(format!("{stem}.stdout.log"));
    let err_log = logs.join(format!("{stem}.stderr.log"));

    let java = env::var("IJ_JAVA").unwrap_or_else(|_| "java".into());
    let mut c = Command::new(java);
    if headless {
        c.arg("-Djava.awt.headless=true");
    }
    c.arg(format!("-Duser.home={}", fwd(&home)))
        // LF line endings in CSV / text-image output on every OS (ImageJ uses the platform separator otherwise)
        .arg("-Dline.separator=\n")
        .arg("-Dfile.encoding=UTF-8")
        .arg("-Dstdout.encoding=UTF-8")
        .arg("-jar")
        .arg(jar)
        .arg("-batch")
        .arg(fwd(macro_path))
        .arg(arg)
        .current_dir(&home)
        .stdin(Stdio::null())
        .stdout(fs::File::create(&out_log).map_err(|e| e.to_string())?)
        .stderr(fs::File::create(&err_log).map_err(|e| e.to_string())?);
    let mut child = c.spawn().map_err(|e| format!("cannot start java: {e}"))?;
    let timeout = Duration::from_secs(
        env::var("IJ_TIMEOUT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(120),
    );
    let t0 = Instant::now();
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(_) => break, // NB: ImageJ exits 0 even when the macro aborted; the sentinel decides.
            None if t0.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{stem}: timed out after {timeout:?} (macro error dialogs are modal and block; see {})",
                    fwd(&out_log)
                ));
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    let so = String::from_utf8_lossy(&fs::read(&out_log).unwrap_or_default()).into_owned();
    let se = String::from_utf8_lossy(&fs::read(&err_log).unwrap_or_default()).into_owned();
    Ok((so, se))
}

fn generate(root: &Path, out: &Path) -> Result<Run, String> {
    let jar = env::var("IJ_JAR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join("../reference/ij/ij-1.54p.jar"));
    let jar = jar
        .canonicalize()
        .map_err(|e| format!("ij jar not found at {}: {e}", jar.display()))?;
    let jar = PathBuf::from(fwd(&jar));
    let jar_sha256 = sha256_file(&jar).map_err(|e| e.to_string())?;
    if jar_sha256 != PINNED_JAR_SHA256 && !env::var("IJ_ALLOW_UNPINNED").is_ok_and(|v| v == "1") {
        return Err(format!(
            "{} has sha256 {jar_sha256}, not the pinned ij-1.54p hash; set IJ_ALLOW_UNPINNED=1 to use it anyway",
            jar.display()
        ));
    }
    let headless = env::var("IJ_HEADLESS").is_ok_and(|v| v == "1");

    if out.exists() {
        fs::remove_dir_all(out).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let out_arg = fwd(&out.canonicalize().map_err(|e| e.to_string())?);

    // Ask ImageJ itself for its version.
    let probe = root.join("target").join("version-probe.ijm");
    fs::create_dir_all(probe.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(&probe, "print(\"IJ_VERSION=\"+getVersion());\nprint(\"JAVA_VERSION=\"+getInfo(\"java.version\"));\n")
        .map_err(|e| e.to_string())?;
    let (so, _) = run_macro(root, &jar, &probe, "", headless)?;
    let grab = |key: &str| {
        so.lines()
            .find_map(|l| l.trim().strip_prefix(key).map(str::to_owned))
            .unwrap_or_else(|| "unknown".into())
    };
    let (ij_version, java_version) = (grab("IJ_VERSION="), grab("JAVA_VERSION="));
    println!("ImageJ {ij_version} on Java {java_version} (headless flag: {headless})");

    let mut macros: Vec<PathBuf> = fs::read_dir(root.join("fixtures/imagej/gen"))
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "ijm"))
        .collect();
    macros.sort();

    let mut files = BTreeMap::new();
    let mut before = snapshot(out).map_err(|e| e.to_string())?;
    for m in &macros {
        let name = m.file_name().unwrap().to_string_lossy().into_owned();
        let t0 = Instant::now();
        let (so, se) = run_macro(root, &jar, m, &out_arg, headless)?;
        if !so.lines().any(|l| l.trim() == SENTINEL) {
            return Err(format!(
                "{name}: did not print {SENTINEL} (macro aborted).\n--- stdout ---\n{so}\n--- stderr (first lines) ---\n{}",
                se.lines().take(12).collect::<Vec<_>>().join("\n")
            ));
        }
        let after = snapshot(out).map_err(|e| e.to_string())?;
        let mut n = 0;
        for (f, (sha, len)) in &after {
            if before.get(f).map(|(s, _)| s) != Some(sha) {
                files.insert(
                    f.clone(),
                    Entry {
                        sha256: sha.clone(),
                        bytes: *len,
                        macro_name: format!("gen/{name}"),
                    },
                );
                n += 1;
            }
        }
        println!(
            "  {name}: {n} file(s) in {:.1}s",
            t0.elapsed().as_secs_f32()
        );
        before = after;
    }

    let run = Run {
        files,
        ij_version,
        java_version,
        jar_sha256,
        jar_name: jar.file_name().unwrap().to_string_lossy().into_owned(),
        headless,
    };
    fs::write(out.join("manifest.toml"), manifest(&run)).map_err(|e| e.to_string())?;
    println!(
        "wrote {} files + manifest.toml to {}",
        run.files.len(),
        out.display()
    );
    Ok(run)
}

fn manifest(run: &Run) -> String {
    // Deliberately contains no timestamps, host names or absolute paths so that it is itself reproducible.
    let mut s = String::new();
    s.push_str("# Generated by `cargo xtask gen` - do not edit.\n");
    s.push_str("[generator]\n");
    writeln!(s, "imagej_version = \"{}\"", run.ij_version).unwrap();
    writeln!(s, "imagej_artifact = \"net.imagej:ij ({})\"", run.jar_name).unwrap();
    writeln!(s, "imagej_jar_sha256 = \"{}\"", run.jar_sha256).unwrap();
    writeln!(s, "java_version = \"{}\"", run.java_version).unwrap();
    writeln!(s, "os = \"{}\"", env::consts::OS).unwrap();
    writeln!(s, "awt_headless = {}", run.headless).unwrap();
    for (name, e) in &run.files {
        s.push_str("\n[[file]]\n");
        writeln!(s, "path = \"{name}\"").unwrap();
        writeln!(s, "sha256 = \"{}\"", e.sha256).unwrap();
        writeln!(s, "bytes = {}", e.bytes).unwrap();
        writeln!(s, "macro = \"{}\"", e.macro_name).unwrap();
        if VOLATILE.contains(&name.as_str()) {
            s.push_str("reproducible = false # embeds zip entry timestamps\n");
        }
    }
    s
}

/// Minimal reader for the manifest we write ourselves (path -> sha256).
fn read_manifest(p: &Path) -> Result<BTreeMap<String, String>, String> {
    let text = fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let val = |l: &str| l.split('"').nth(1).unwrap_or("").to_owned();
    let mut m = BTreeMap::new();
    let mut path = None;
    for l in text.lines() {
        if l.starts_with("path = ") {
            path = Some(val(l));
        } else if l.starts_with("sha256 = ")
            && let Some(p) = path.take()
        {
            m.insert(p, val(l));
        }
    }
    Ok(m)
}

fn verify(root: &Path) -> Result<(), String> {
    let committed = read_manifest(&root.join("fixtures/imagej/out/manifest.toml"))?;
    let tmp = root.join("target").join("verify-out");
    let run = generate(root, &tmp)?;
    let mut bad = 0;
    for (name, sha) in &committed {
        let volatile = VOLATILE.contains(&name.as_str());
        match run.files.get(name) {
            Some(e) if &e.sha256 == sha => println!("  identical  {name}"),
            Some(_) if volatile => {
                println!("  DIFFERS    {name}   (expected: marked reproducible = false)")
            }
            Some(_) => {
                println!("  DIFFERS    {name}");
                bad += 1;
            }
            None => {
                println!("  MISSING    {name}");
                bad += 1;
            }
        }
    }
    for name in run.files.keys().filter(|n| !committed.contains_key(*n)) {
        println!("  NEW        {name}");
        bad += 1;
    }
    if bad > 0 {
        return Err(format!("{bad} file(s) not reproducible"));
    }
    println!(
        "verify: all non-volatile outputs are byte-identical to fixtures/imagej/out/manifest.toml"
    );
    Ok(())
}
