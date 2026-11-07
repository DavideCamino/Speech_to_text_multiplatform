# Install Speech2Text

[Application overview and usage](README.md) · [Build and package from source](BUILD_FROM_SOURCE.md)

Use packages from the project's [GitHub Releases](https://github.com/DavideCamino/Speech_to_text_multiplatform/releases). If a release has no package for your platform, follow the source-build guide; the presence of packaging scripts does not mean a published binary is available.

The packaged targets are **Linux x86_64** and **Windows 10/11 x86_64**, with CPU and Vulkan support. Vulkan needs a compatible GPU and current vendor drivers; CPU mode remains available. Models are not included. You do not need Rust, Python, a Vulkan SDK, or a separate GTK installation to use the Windows package; Flatpak obtains its runtime separately.

Checksums detect corruption, not publisher identity independently of the download source. These packages are unsigned; do not disable security protection to run an unexpected or blocked download.

## Linux: Flatpak

### Install

Install Flatpak using your distribution's package manager. For example:

```sh
# Ubuntu / Debian
sudo apt install flatpak

# Arch Linux
sudo pacman -S flatpak
```

Add Flathub for the required runtime, then download `Speech2Text-x86_64.flatpak` and `SHA256SUMS` from the same release. Run the following from the download directory:

```sh
flatpak remote-add --user --if-not-exists flathub \
  https://flathub.org/repo/flathub.flatpakrepo
sha256sum --check SHA256SUMS
flatpak install --user ./Speech2Text-x86_64.flatpak
flatpak run io.github.davidecamino.Speech2Text
```

Accept installation of the required runtime. After installation, Speech2Text should also appear in your desktop application launcher; log out and back in if this is your first Flatpak setup and it does not appear.

This is a **sideloaded bundle**, not a Flathub application. The sandbox uses desktop file portals, PulseAudio access for microphones, and GPU access. It does not request unrestricted access to your home directory. Keep your desktop portal services and graphics drivers working and up to date; NVIDIA also uses Vulkan in this package, not CUDA.

### Update or uninstall

Download and verify the new bundle, then install it with the same `flatpak install --user ./Speech2Text-x86_64.flatpak` command and accept the update prompt. There is no automatic application-update feed. Runtime updates can be installed with:

```sh
flatpak update --user
```

Remove the app with:

```sh
flatpak uninstall --user io.github.davidecamino.Speech2Text
```

Do not add `--delete-data` unless you intend to remove the app's saved settings and downloaded models too.

## Windows: installer

1. Download `Speech2Text-<version>-windows-x86_64-setup.exe` and `Speech2Text-installer.sha256` from the same release.
2. Optionally verify the installer in PowerShell from the download directory:

   ```powershell
   $entry = ((Get-Content ./Speech2Text-installer.sha256).Trim() -split '\s+', 2)
   (Get-FileHash -LiteralPath $entry[1] -Algorithm SHA256).Hash -eq $entry[0]
   ```

   The result must be `True`. Use the release's exact filename if checking manually.
3. Run the installer. It installs for the current user, by default under `%LOCALAPPDATA%\Programs\Speech2Text`, without requiring administrator installation. It creates a Start Menu entry and offers an optional desktop shortcut.
4. Launch **Speech2Text** from the Start Menu. A console window remains open for diagnostics; this is expected.
5. For recording, enable microphone access for desktop applications in Windows privacy settings.

The installer bundles the GTK runtime and Vulkan loader, **not a GPU driver**. No MSYS2 installation is needed on the target machine. If acceleration is unavailable, disable **Hardware acceleration** in Settings.

### Update or uninstall

Close the app and run the newer installer to update it. There is no automatic update mechanism. Remove Speech2Text through Windows **Settings → Apps** or its uninstaller. Uninstallation leaves preferences and downloaded models in your profile.

### Optional portable ZIP

If you prefer not to install, download `Speech2Text-windows-x86_64.zip` and its matching `.sha256` file. Extract the **entire** archive and run `Speech2Text/Start-Speech2Text.cmd`. Do not launch inside the ZIP or copy only the executable. The portable package still stores preferences and models in your user profile, not beside the launcher.

## First launch

Open **Settings → Speech to Text → Local Whisper model**, download a model or select an existing whisper.cpp `.bin` file, then choose a language and open an audio file. Start with Tiny or Base and CPU mode if you are unsure about hardware support. See [Usage](README.md#usage) for recording, editing, subtitle export, and optional LLM rewriting.

## Saved data

Default locations:

| Platform | Settings | Downloaded models |
| --- | --- | --- |
| Linux Flatpak | `~/.var/app/io.github.davidecamino.Speech2Text/config/speech2text/settings.json` | `~/.var/app/io.github.davidecamino.Speech2Text/data/speech2text/models/` |
| Linux native build | `~/.config/speech2text/settings.json` | `~/.local/share/speech2text/models/` |
| Windows | `%APPDATA%\Speech2Text\config\settings.json` | `%APPDATA%\Speech2Text\data\models\` |

Native Linux paths respect `XDG_CONFIG_HOME` and `XDG_DATA_HOME`. Models selected with **Use Original** stay at their original location. Exports go to your selected destination; transcripts and rewrite results are not automatically saved.

Older Windows builds used `%APPDATA%\Davide Camino\Speech2Text\`. There is no automatic migration: with the app closed, copy the old `config` and `data` folders into `%APPDATA%\Speech2Text\` if needed, backing up any existing files first. Reselect the model afterward if its saved path points to the old folder.

The Windows launcher also maintains a regenerable image-loader cache under `%LOCALAPPDATA%\io.github.davidecamino.Speech2Text\cache`.

## Troubleshooting

- **No model selected:** select or download one in Settings; packages do not contain models.
- **Microphone missing:** check OS permissions, choose the correct input, and use **Refresh** after device changes.
- **Slow recording or queue overflow:** try Tiny, a faster GPU backend, or file transcription instead.
- **GPU errors:** update the vendor driver or switch to CPU. Installing a Vulkan SDK is not required to run packaged builds.
- **Flatpak file dialog issues:** check your desktop's portal backend. Choose files through the app's file chooser rather than granting broad filesystem permissions.
- **Windows startup errors:** retain the console message and inspect the package's `runtime-manifest.json` and `licenses/msys2-packages.txt`.

Report the app version, OS, installation method, and relevant errors in [GitHub Issues](https://github.com/DavideCamino/Speech_to_text_multiplatform/issues). Do not include private audio, transcripts, prompts, or authentication tokens without reviewing them first.
