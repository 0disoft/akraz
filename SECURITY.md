# Security

## Reporting

Please report security issues privately through GitHub Security Advisories when available, or by opening a minimal private maintainer contact path before publishing exploit details.

## Known Dependency Advisory

### GHSA-wrw7-89jp-8q8g / RUSTSEC-2024-0429

GitHub Dependabot reports `glib` `0.18.x` through the Tauri 2 Linux GTK/WebKit dependency graph:

`akraz-app -> tauri 2.11.2 -> webkit2gtk/gtk 0.18.x -> glib 0.18.x`

The advisory is about unsound `glib::VariantStrIter` iterator implementations. The fixed advisory range starts at `glib` `0.20.0`, but Tauri 2.11.2 still resolves GTK 3 bindings that require `glib` `0.18.x`. A direct `glib` bump is rejected by Cargo because `gtk 0.18.x` requires `glib ^0.18`.

Akraz is currently a Windows MVP. The Windows target graph does not include `glib`, GTK, or WebKitGTK, so released Windows builds are not affected by this advisory. Linux GUI builds remain unsupported until the project reaches the Linux stage of the roadmap.

Do not treat this as permanently resolved. Revisit this advisory before enabling or publishing Linux GUI builds, when Tauri moves to a GTK/WebKit stack that allows `glib >= 0.20.0`, or when Akraz adopts a different Linux desktop runtime. The expected resolution is to move the Tauri Linux GUI dependency graph off the vulnerable `glib 0.18.x` line, not to pin a local fork silently.
