use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const APP_NAME: &str = "Editable";
const BUNDLE_ID: &str = "dev.local.editable";

fn main() -> io::Result<()> {
    let command = env::args().nth(1).unwrap_or_else(|| "bundle".to_string());
    match command.as_str() {
        "bundle" => bundle(),
        "icon" => generate_icon(),
        _ => {
            eprintln!("usage: cargo run -p editable-build-tools -- [bundle|icon]");
            std::process::exit(2);
        }
    }
}

fn bundle() -> io::Result<()> {
    let root = workspace_root();
    let dist = root.join("dist");
    let app = dist.join(format!("{APP_NAME}.app"));
    let contents = app.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    fs::create_dir_all(&macos)?;
    fs::create_dir_all(&resources)?;

    let binary = root.join("target/release/editable");
    if !binary.exists() {
        eprintln!("missing release binary at {}", binary.display());
        eprintln!("run: cargo build --release -p editable");
        std::process::exit(1);
    }

    fs::copy(&binary, macos.join(APP_NAME))?;
    let icon = root.join("assets/icon/Editable.icns");
    if icon.exists() {
        fs::copy(icon, resources.join("Editable.icns"))?;
    } else {
        eprintln!("warning: assets/icon/Editable.icns not found; run the icon command");
    }

    fs::write(contents.join("Info.plist"), info_plist())?;
    fs::write(contents.join("PkgInfo"), "APPL????")?;
    println!("{}", app.display());
    Ok(())
}

fn generate_icon() -> io::Result<()> {
    let root = workspace_root();
    let source = root.join("assets/icon/editable-icon.png");
    let iconset = root.join("assets/icon/Editable.iconset");
    let icns = root.join("assets/icon/Editable.icns");
    if !source.exists() {
        eprintln!("missing source icon at {}", source.display());
        std::process::exit(1);
    }
    fs::create_dir_all(&iconset)?;

    let sizes = [
        ("icon_16x16.png", 16),
        ("icon_16x16@2x.png", 32),
        ("icon_32x32.png", 32),
        ("icon_32x32@2x.png", 64),
        ("icon_128x128.png", 128),
        ("icon_128x128@2x.png", 256),
        ("icon_256x256.png", 256),
        ("icon_256x256@2x.png", 512),
        ("icon_512x512.png", 512),
        ("icon_512x512@2x.png", 1024),
    ];

    for (name, size) in sizes {
        run(
            "sips",
            &[
                "-z",
                &size.to_string(),
                &size.to_string(),
                source.to_str().unwrap(),
                "--out",
                iconset.join(name).to_str().unwrap(),
            ],
        )?;
    }
    if let Err(err) = run(
        "iconutil",
        &[
            "-c",
            "icns",
            "-o",
            icns.to_str().unwrap(),
            iconset.to_str().unwrap(),
        ],
    ) {
        eprintln!("iconutil failed ({err}); writing a PNG-backed ICNS fallback");
        write_icns_fallback(&iconset, &icns)?;
    }
    println!("{}", icns.display());
    Ok(())
}

fn run(program: &str, args: &[&str]) -> io::Result<()> {
    let status = Command::new(program).args(args).status()?;
    if !status.success() {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("{program} failed with {status}"),
        ))
    } else {
        Ok(())
    }
}

fn write_icns_fallback(iconset: &Path, output: &Path) -> io::Result<()> {
    let entries = [
        ("icp4", "icon_16x16.png"),
        ("icp5", "icon_32x32.png"),
        ("icp6", "icon_32x32@2x.png"),
        ("ic07", "icon_128x128.png"),
        ("ic08", "icon_256x256.png"),
        ("ic09", "icon_512x512.png"),
        ("ic10", "icon_512x512@2x.png"),
    ];
    let mut chunks = Vec::new();
    for (kind, file) in entries {
        let data = fs::read(iconset.join(file))?;
        let mut chunk = Vec::with_capacity(data.len() + 8);
        chunk.extend_from_slice(kind.as_bytes());
        chunk.extend_from_slice(&(data.len() as u32 + 8).to_be_bytes());
        chunk.extend_from_slice(&data);
        chunks.extend_from_slice(&chunk);
    }

    let mut out = Vec::with_capacity(chunks.len() + 8);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&(chunks.len() as u32 + 8).to_be_bytes());
    out.extend_from_slice(&chunks);
    fs::write(output, out)
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("build-tools lives under workspace root")
        .to_path_buf()
}

fn info_plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleExecutable</key>
  <string>{APP_NAME}</string>
  <key>CFBundleIconFile</key>
  <string>Editable</string>
  <key>CFBundleIdentifier</key>
  <string>{BUNDLE_ID}</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>{APP_NAME}</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSPrincipalClass</key>
  <string>NSApplication</string>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeExtensions</key>
      <array><string>csv</string></array>
      <key>CFBundleTypeIconFile</key>
      <string>Editable</string>
      <key>CFBundleTypeName</key>
      <string>CSV Document</string>
      <key>CFBundleTypeRole</key>
      <string>Editor</string>
      <key>LSHandlerRank</key>
      <string>Owner</string>
      <key>LSItemContentTypes</key>
      <array><string>public.comma-separated-values-text</string></array>
    </dict>
  </array>
  <key>UTExportedTypeDeclarations</key>
  <array>
    <dict>
      <key>UTTypeIdentifier</key>
      <string>public.comma-separated-values-text</string>
      <key>UTTypeDescription</key>
      <string>CSV document</string>
      <key>UTTypeConformsTo</key>
      <array><string>public.delimited-values-text</string></array>
      <key>UTTypeTagSpecification</key>
      <dict>
        <key>public.filename-extension</key>
        <array><string>csv</string></array>
        <key>public.mime-type</key>
        <string>text/csv</string>
      </dict>
    </dict>
  </array>
</dict>
</plist>
"#
    )
}
