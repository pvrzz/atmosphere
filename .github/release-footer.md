<!-- install-notes -->

### Downloads

| File | What it is |
| --- | --- |
| `Atmosphere_*_x64-setup.exe` | The installer (recommended). Updates itself from then on. |
| `Atmosphere_*_x64_en-US.msi` | The same app as an MSI, for managed installs. |
| `Atmosphere_*_x64-portable.exe` | No install. Keeps its data beside the file when you add a `portable` file next to it. |

Atmosphere runs on Windows 10 and 11 (64-bit).

> [!WARNING]
> **Atmosphere isn't code-signed yet**, so Windows SmartScreen may say "Windows protected your PC", and some antivirus apps may flag it as a false positive. To install anyway, choose **More info → Run anyway**.
> Every file here is built from this repository's source by GitHub Actions, and in-app updates are still signature-checked before they install.
