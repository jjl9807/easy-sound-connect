# Application identity

Use `Easy Sound Connect` as the product display name and `com.jjl9807.ezsc` as the application identifier. The runtime constants are defined in `crates/app/src/lib.rs`. The executable remains `easy-sound-connect`; the identifier is not an executable name or a per-window numeric ID.

## Linux

Install [com.jjl9807.ezsc.desktop](linux/com.jjl9807.ezsc.desktop) into the installation prefix's `share/applications/` directory, and place the executable in `bin/` on the desktop session's PATH. A desktop entry installed under the XDG user applications directory is also supported. The desktop file name and `StartupWMClass` must match the Wayland `app_id` and X11 `WM_CLASS`: `com.jjl9807.ezsc`. `Name` is always `Easy Sound Connect`. The application does not install this launcher automatically.

Install [app_logo.svg](../crates/app/assets/app_logo.svg) as `share/icons/hicolor/scalable/apps/com.jjl9807.ezsc.svg` under the same prefix, or under the XDG user data directory for a user installation. The desktop entry's `Icon=com.jjl9807.ezsc` resolves this full-color application icon. Update the distribution's icon cache as part of package installation when required. X11 windows also expose a rasterized copy of the same logo directly; Wayland desktop shells resolve it through the installed desktop entry. The monochrome tray assets are separate and follow the tray host's appearance.

An autostart file alone does not register the application with the desktop shell. Without the installed application desktop entry, a shell may display the raw application identifier even when the window title is correct. Login startup uses `com.jjl9807.ezsc.desktop` in the user's autostart directory, with the current executable's absolute path and `--background`.

## macOS

An application bundle must set `CFBundleIdentifier` to `com.jjl9807.ezsc`, both `CFBundleName` and `CFBundleDisplayName` to `Easy Sound Connect`, and `CFBundleExecutable` to `easy-sound-connect`. Supply the release version, Bluetooth usage description and other platform requirements when constructing the bundle. Notifications use this bundle identifier. The login LaunchAgent uses the same identifier as its Label and `com.jjl9807.ezsc.plist` as its filename.

Generate the bundle's `.icns` resource from `crates/app/assets/app_logo.svg` and reference it with `CFBundleIconFile`. The source SVG is also used by the application's About page.

## Windows

Use `Easy Sound Connect` for product, shortcut and application display names. Unpackaged processes, notification registration and shortcuts use `com.jjl9807.ezsc` as the AppUserModelID. The login Run value uses the same identifier. Windows packages derive their effective AUMID from the package identity; package manifests and shortcuts must follow that platform format while retaining the product name and identifier as their naming basis.

Generate a multi-resolution `.ico` from `crates/app/assets/app_logo.svg` for executable and shortcut resources. The current GPUI Windows backend loads the executable's icon resource with numeric ID `1`; packaged application visual assets must use the same logo.

These files define metadata conventions and the Linux launcher; they do not build complete platform installers.
