# Application identity

Use `Easy Sound Connect` as the product display name and `com.jjl9807.ezsc` as the application identifier. The runtime constants are defined in `crates/app/src/lib.rs`. The executable remains `easy-sound-connect`; the identifier is not an executable name or a per-window numeric ID.

## Linux

Install [com.jjl9807.ezsc.desktop](linux/com.jjl9807.ezsc.desktop) into the installation prefix's `share/applications/` directory, and place the executable in `bin/` on the desktop session's PATH. A desktop entry installed under the XDG user applications directory is also supported. The desktop file name and `StartupWMClass` must match the Wayland `app_id` and X11 `WM_CLASS`: `com.jjl9807.ezsc`. `Name` is always `Easy Sound Connect`. The application does not install this launcher automatically.

An autostart file alone does not register the application with the desktop shell. Without the installed application desktop entry, a shell may display the raw application identifier even when the window title is correct. Login startup uses `com.jjl9807.ezsc.desktop` in the user's autostart directory, with the current executable's absolute path and `--background`.

## macOS

An application bundle must set `CFBundleIdentifier` to `com.jjl9807.ezsc`, both `CFBundleName` and `CFBundleDisplayName` to `Easy Sound Connect`, and `CFBundleExecutable` to `easy-sound-connect`. Supply the release version, Bluetooth usage description and other platform requirements when constructing the bundle. Notifications use this bundle identifier. The login LaunchAgent uses the same identifier as its Label and `com.jjl9807.ezsc.plist` as its filename.

## Windows

Use `Easy Sound Connect` for product, shortcut and application display names. Unpackaged processes, notification registration and shortcuts use `com.jjl9807.ezsc` as the AppUserModelID. The login Run value uses the same identifier. Windows packages derive their effective AUMID from the package identity; package manifests and shortcuts must follow that platform format while retaining the product name and identifier as their naming basis.

These files define metadata conventions and the Linux launcher; they do not build complete platform installers.
