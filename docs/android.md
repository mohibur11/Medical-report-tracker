# Building for Android

The Rust backend cross-compiles and packages into an APK. What is *in* that APK
is another matter — see "What does not work yet" at the bottom.

## What is installed

| | |
|---|---|
| JDK 21 (Temurin) | Gradle does not support the JDK 26 that is this machine's default, so Android builds point at 21 explicitly and `JAVA_HOME` is left alone |
| Android SDK 35, build-tools 35 | `%LOCALAPPDATA%\Android\Sdk` |
| NDK 29 | needed by `ring`, and by anything else that compiles C |
| Four Rust targets | `aarch64-linux-android` is the one that matters; the rest are for emulators and old devices |

`ANDROID_HOME`, `NDK_HOME` and `ANDROID_JAVA_HOME` are set as user environment
variables.

## Building

```powershell
$env:ANDROID_HOME = "$env:LOCALAPPDATA\Android\Sdk"
$env:NDK_HOME = "$env:ANDROID_HOME\ndk\29.0.14206865"
$env:JAVA_HOME = "C:\Program Files\Eclipse Adoptium\jdk-21.0.12.8-hotspot"

npm run tauri android build -- --debug --target aarch64
```

That will get as far as compiling the library and then fail on a symbolic link.
Finish it by hand:

```powershell
$strip = "$env:NDK_HOME\toolchains\llvm\prebuilt\windows-x86_64\bin\llvm-strip.exe"
Copy-Item src-tauri\target\aarch64-linux-android\debug\libmrt_lib.so `
          src-tauri\gen\android\app\src\main\jniLibs\arm64-v8a\libmrt_lib.so -Force
& $strip --strip-all src-tauri\gen\android\app\src\main\jniLibs\arm64-v8a\libmrt_lib.so

cd src-tauri\gen\android
.\gradlew.bat assembleArm64Debug -x :app:rustBuildArm64Debug --no-daemon
```

Output: `app\build\outputs\apk\arm64\debug\app-arm64-debug.apk`, about 33 MB.

## Four things that will waste your afternoon

**`cargo build --target aarch64-linux-android` on its own fails.** `ring` needs
the NDK's clang and the linker environment that goes with it, and only the Tauri
CLI sets those up. Compile through the CLI even though its last step fails.

**The CLI fails creating a symbolic link into `jniLibs`.** Windows refuses
symlinks unless Developer Mode is on. Enabling it fixes this properly and is
required for `tauri android dev`; copying the library by hand is the way around
it otherwise.

**`:app:rustBuildArm64Debug` panics if you let it run.** It re-invokes the CLI
expecting a dev-server address file that only `android dev` writes. Skip it with
`-x` — the library is already in place by then.

**Strip the library.** A debug build is 208 MB unstripped and 26 MB stripped, and
the difference is entirely symbols. If an APK comes out unexpectedly large, clear
`app\build\intermediates\merged_native_libs` and `app\build\outputs\apk` and
repackage: Gradle can leave stale data inside the archive, producing a 216 MB
file whose contents measure 42 MB.

## Sharing a report to the app

The app appears in Android's share sheet for images and PDFs. Sharing one copies
it into the app's own storage — the web view and `std::fs` can do nothing with a
`content://` URI, so the activity pulls the bytes out first — and the app imports
it the next time it comes to the foreground, which is immediately, because
sharing brings it forward.

Shared files go through exactly the same ingest as a file chosen from disk:
hashed, de-duplicated, straightened. There is no second path for them.

## Installing

Copy the APK to the phone and open it in Files. It is debug-signed, which is what
makes it installable at all — a release build comes out unsigned and Android
refuses it. Expect a Play Protect warning about an unknown developer.

## What does not work yet

- **OCR is wired to ML Kit and unverified on a device.** `Windows.Media.Ocr` has
  no Android equivalent, so `OcrPlugin.kt` uses ML Kit's on-device Latin
  recogniser — bundled rather than the Play-services variant, so it works with no
  Google Play and makes no network call. It returns the same text and word boxes
  the Windows recogniser does, which is what lets the date ranker behave
  identically on both. It compiles and packages; whether it recognises a real
  report has not been tested.
- **PDFs are not read on Android.** Page images are extracted with pdfcpu, which
  cannot run there, so recognition covers photographs only.
- **Google Drive sign-in should work, and is unverified.** The loopback listener
  binds inside the app and the browser on the same phone can reach it, so the
  desktop OAuth client is reused as-is. Opening the browser goes through the
  opener plugin rather than the Windows-only call it used before. Whether Chrome
  Custom Tabs will follow a `http://127.0.0.1:PORT` redirect back to the app has
  not been tested on a device.
- **The refresh token is not encrypted on Android.** On Windows DPAPI protects one
  row in a database that anyone with the user's files can read. Android does not
  have that problem — the database is in internal app storage, which no other app
  and no file manager can reach. The sandbox is the boundary instead. That is
  weaker than DPAPI against a rooted phone or a full-device backup, and Android
  Keystore would close the gap; it needs a JNI binding this app does not have.
- **Export fails.** All PDF assembly runs through the pdfcpu sidecar, and Android
  does not permit executing bundled binaries. Merged export is deliberately a
  desktop feature.

The vault lives in `/Android/data/com.mohibur.medicinereporttracker/files/Documents`,
which is readable over USB and **deleted when the app is uninstalled**. That is
survivable only because the Drive copy is the one that matters, which is why
backing up is not optional on a phone.
