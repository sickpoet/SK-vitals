// 把应用图标和版本信息编进 exe。
// 依赖 Windows SDK 里的 rc.exe（MSVC 工具链自带），找不到时静默跳过，不影响编译。
fn main() {
    println!("cargo:rerun-if-changed=assets/sk-vitals.ico");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest = r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity version="0.1.0.0" processorArchitecture="*" name="SK-vitals" type="win32"/>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
</assembly>"#;

    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/sk-vitals.ico")
        .set("ProductName", "SK-vitals")
        .set("FileDescription", "SK-vitals 任务栏 CPU / 内存 / 温度监控")
        .set("CompanyName", "SickPoet")
        .set("LegalCopyright", "Copyright (c) 2026 柯夜")
        .set("InternalName", "sk-vitals")
        .set("OriginalFilename", "sk-vitals.exe")
        .set_manifest(manifest);

    if let Err(e) = res.compile() {
        println!("cargo:warning=编译资源失败，exe 将不带图标: {e}");
    }
}
