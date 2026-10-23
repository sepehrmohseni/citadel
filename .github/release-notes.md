## Downloads

| Platform | Installer |
| --- | --- |
| Windows 10 and 11, x64 | [Citadel_@VERSION@_x64-setup.exe](https://github.com/sepehrmohseni/citadel/releases/download/v@VERSION@/Citadel_@VERSION@_x64-setup.exe) or [the MSI](https://github.com/sepehrmohseni/citadel/releases/download/v@VERSION@/Citadel_@VERSION@_x64_en-US.msi) |
| macOS 11 or later, Apple Silicon and Intel | [Citadel_@VERSION@_universal.dmg](https://github.com/sepehrmohseni/citadel/releases/download/v@VERSION@/Citadel_@VERSION@_universal.dmg) |
| Debian, Ubuntu | [Citadel_@VERSION@_amd64.deb](https://github.com/sepehrmohseni/citadel/releases/download/v@VERSION@/Citadel_@VERSION@_amd64.deb) |
| Fedora, RHEL, openSUSE | [Citadel-@VERSION@-1.x86_64.rpm](https://github.com/sepehrmohseni/citadel/releases/download/v@VERSION@/Citadel-@VERSION@-1.x86_64.rpm) |
| Any Linux | [Citadel_@VERSION@_amd64.AppImage](https://github.com/sepehrmohseni/citadel/releases/download/v@VERSION@/Citadel_@VERSION@_amd64.AppImage) |
| Android 7 or later | [Citadel_@VERSION@_android.apk](https://github.com/sepehrmohseni/citadel/releases/download/v@VERSION@/Citadel_@VERSION@_android.apk) |

Or install from a terminal:

```sh
curl -fsSL https://sepehrmohseni.github.io/citadel/install.sh | sh
```

```powershell
irm https://sepehrmohseni.github.io/citadel/install.ps1 | iex
```

**Android:** open the APK on the phone and allow installs from your browser when Android asks. The APK is signed with the project key; its certificate SHA-256 is `73d60e8e634f8139367d2a93c43979735391c675a0c3c3f6fc646869b3c8967a`.

Every file is listed in `SHA256SUMS`. Desktop builds are not code-signed yet, so Windows and macOS warn on first launch; [INSTALL.md](https://github.com/sepehrmohseni/citadel/blob/master/docs/INSTALL.md) explains what to click and how to verify a download.

