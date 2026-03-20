# NexiBot Icons Guide

## Icon Files and Their Purpose

### Menubar/Tray Icon (macOS)
**File:** `tray-icon.png`
- **Purpose:** Menubar tray icon (top-right corner of screen)
- **Design:** Black brain circuit on transparent background
- **Technical:** Template icon that inverts with macOS theme
- **Usage in code:** `include_bytes!("../icons/tray-icon.png")`
- **Must use with:** `icon_as_template(true)` in TrayIconBuilder

### App Icon (Dock)
**Files:** `icon.icns`, `icon.ico`, `*.png` (various sizes)
- **Purpose:** Application icon in dock, Finder, etc.
- **Design:** Blue/light background with black brain circuit
- **Usage:** Automatically loaded by Tauri from tauri.conf.json

## Common Mistakes to Avoid

### ❌ WRONG: Using app icon for tray
```rust
// DON'T DO THIS:
let icon = app.default_window_icon().unwrap().clone();
```
**Problem:** This loads the dock icon (blue background), not the menubar template icon.

### ✅ CORRECT: Using tray template icon
```rust
// DO THIS:
let tray_icon_bytes = include_bytes!("../icons/tray-icon.png");
let tray_icon = Image::from_bytes(tray_icon_bytes)?;
```

## Icon Requirements

### Menubar Template Icon
- ✅ Black or dark design on transparent background
- ✅ Simple, recognizable at 16-22px
- ✅ Used with `icon_as_template(true)`
- ✅ macOS will invert colors automatically for light/dark mode

### App Icon
- ✅ Colorful, detailed design
- ✅ Multiple sizes (16x16 to 512x512)
- ✅ Should look good in both dock and Finder

## Troubleshooting

**Problem:** White square or wrong icon in menubar
- **Cause:** Using wrong icon file (probably app icon)
- **Fix:** Ensure you're loading `tray-icon.png` with `include_bytes!`

**Problem:** Multiple tray icons appearing
- **Cause:** Icon defined in both tauri.conf.json AND main.rs
- **Fix:** Only create tray icon programmatically in main.rs (not in config)

**Problem:** Icon doesn't invert with dark mode
- **Cause:** Not using `icon_as_template(true)`
- **Fix:** Add `.icon_as_template(true)` to TrayIconBuilder

## References

- [Tauri Tray Icon Docs](https://tauri.app/v1/guides/features/system-tray/)
- [macOS Template Images](https://developer.apple.com/design/human-interface-guidelines/images#Template-images)
