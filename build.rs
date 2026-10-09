// Embed a Windows application manifest requesting administrator rights.
//
// A remote-input tool needs elevation: Windows UIPI (User Interface
// Privilege Isolation) silently drops mouse/keyboard input sent from a
// non-elevated process to an elevated window (Task Manager, admin prompts,
// installers...). Without this manifest the app appears to "stop working"
// whenever such a window is in the foreground.
fn main() {
    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_manifest(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="requireAdministrator" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>"#,
        );
        if let Err(e) = res.compile() {
            eprintln!("winres manifest embed failed: {e}");
            std::process::exit(1);
        }
    }
}
