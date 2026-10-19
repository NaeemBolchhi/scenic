# Scenic

Android wallpaper app on Tauri v2. The frontend is a single `src/index.html` with no build step. The backend saves images to the gallery and sets wallpapers.

## Layout

```
src/index.html                 Frontend (edit freely; keep the Scenic API block)
src-tauri/                     The app crate
  src/commands.rs              download_wallpaper, set_wallpaper, and the four image-cache commands
  src/image.rs                 Data URI parsing, base64 decode, format sniffing, file names
  src/cache.rs                 Image cache: entries, creation/expiry times, erase, expire
  src/fetch.rs                 Image downloads (http/https, size-capped)
  src/temp.rs                  Temp files in the cache dir (auto-deleted)
  src/error.rs                 Errors sent to JS as { kind, message }
  tauri.conf.json              Name, identifier (nb.scenic), minSdk 29
tauri-plugin-scenic/           Local plugin: the Android bridge
  src/android.rs               Calls into Kotlin
  src/unsupported.rs           Desktop/iOS stub (returns "unsupported")
  android/.../ScenicPlugin.kt  MediaStore + WallpaperManager
app-icon.png                   Placeholder icon source
```

## How a call flows

1. JS passes a base64 data URI to a Rust command.
2. Rust decodes it, checks the real format from the file's magic bytes, enforces a 64 MB limit, and writes the bytes to a temp file in the app cache.
3. Rust hands the file path to Kotlin. Passing a path avoids pushing megabytes of JSON through JNI.
4. Kotlin writes to `Pictures/Scenic/` through MediaStore, or applies the file with `WallpaperManager.setStream`, on a background thread.
5. The temp file is deleted when the command returns, and any leftovers are cleared at startup.

## JS API

```js
await Scenic.downloadWallpaper(dataUri, 'optional-name');
// -> { uri, fileName, relativePath: "Pictures/Scenic/", mimeType, bytes }

await Scenic.setWallpaper(dataUri, 'home' | 'lock' | 'both');   // default 'both'
// -> { target, mimeType, bytes }

const dataUri = await Scenic.toDataUri('https://...');           // helper for URLs/Blobs
```

Without the wrapper: `invoke('download_wallpaper', { dataUri, fileName })` and `invoke('set_wallpaper', { dataUri, target })`.

Accepted input: `data:image/...;base64,...` or bare base64. Whitespace, missing padding and URL-safe base64 are fine. Formats: JPEG, PNG, WebP, GIF, BMP, HEIC/HEIF, AVIF (AVIF needs Android 12+ to display). The saved file's extension always matches the real format.

Errors reject with `{ kind, message }`, where `kind` is one of `invalidImage`, `invalidArgument`, `tooLarge`, `io`, `network`, `android`, `unsupported`, `internal`.

## Image cache

Downloaded images are kept in the app's cache folder (`<cache dir>/image-cache/thumbnails/` and `.../fullres/`), so Android's Settings > Apps > Scenic > Storage > **Clear cache** wipes them. Each entry is one file holding the image plus its URL, creation time (stamped automatically) and expiry time. Writes are atomic, and storing the same URL and type again replaces the old entry.

```js
// imageType: 'thumbnail' | 'fullres'  ('any' is also accepted by erase/expire)
await Scenic.storeImageCache('thumbnail', url, 7);          // fetch + cache for 7 days
// -> { imageType, url, createdAt, expiresAt, bytes, mimeType }   (times are Unix ms)

await Scenic.eraseImageCache('thumbnail', 3);               // thumbnails created 3+ days ago; fullres untouched
await Scenic.eraseImageCache('any', 0);                     // everything
// -> { removed, freedBytes }

await Scenic.clearExpiredImageCache('any');                 // creation time + lifetime has passed
// -> { removed, freedBytes }

img.src = await Scenic.loadImageCache('thumbnail', url, 7);        // cache first, fetch + cache on a miss
img.src = await Scenic.loadImageCache('thumbnail', url, 7, true);  // forceRefresh: skip the cache, fetch anew, re-cache
```

Notes:

- `days` is the lifetime for a newly written entry. A cache hit keeps the lifetime it was stored with, and an expired entry counts as a miss (it is refetched).
- Raw invokes use camelCase argument names: `invoke('load_image_cache', { imageType, url, days, forceRefresh })`.
- Cache files are never listed in the gallery and are separate from images saved with `downloadWallpaper`.
- Downloads are done in Rust (http/https only, 64 MB cap, 15 s connect and 120 s total timeouts). The app needs the `INTERNET` permission, which the generated Android project includes.

## Setup

Requirements: Rust, Node, Android Studio with SDK + NDK, `JAVA_HOME`, `ANDROID_HOME` and `NDK_HOME` set (see https://v2.tauri.app/start/prerequisites/).

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android
npm install
npm run android:init                  # creates src-tauri/gen/android
npx tauri icon app-icon.png           # copies launcher icons into gen/android
npm run android:dev                   # run on a device or emulator
npm run android:build                 # release APK
npm run test:rust                     # unit tests (run on desktop)
```

`npx tauri dev` also works on desktop for editing the frontend. The two functions return an `unsupported` error there.

## CI (GitHub Actions)

`.github/workflows/build.yml` is one manual job: open the repo's Actions tab, pick **Build**, press **Run workflow**. It never runs on push. When it finishes, download the `android` artifact: a zip holding `Scenic-universal.apk` plus one APK per CPU type (`arm64-v8a`, `armeabi-v7a`, `x86`, `x86_64`).

Signing: with repository secrets `ANDROID_KEYSTORE_B64` (`base64 -w0 release.keystore`), `ANDROID_KEYSTORE_PASSWORD` and `ANDROID_KEY_ALIAS`, the APKs are release-signed with your key. Without them the job builds debug-signed APKs, which install fine but are not meant for distribution.

## Notes

- Permissions: only `SET_WALLPAPER`, which is granted at install. Android 10+ needs no storage permission to add images to the gallery.
- Target `home`: if the lock screen was sharing the old wallpaper, Android keeps that old image on the lock screen. Only the home screen changes.
- Cropping: Android centre-crops the image to the screen. Send images at roughly screen resolution for the sharpest result.
- Renaming: the gallery folder comes from `productName` in `tauri.conf.json`. Changing the package means changing `identifier` there and running `android:init` again.
- Some launchers and device-owner policies block wallpaper changes. That comes back as an `android` error with the reason.
