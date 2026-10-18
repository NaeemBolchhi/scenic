package nb.scenic.plugin

import android.app.Activity
import android.app.WallpaperManager
import android.content.ContentValues
import android.net.Uri
import android.os.Environment
import android.provider.MediaStore
import android.util.Log
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.io.FileInputStream
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

private const val TAG = "ScenicPlugin"

@InvokeArg
class SaveToGalleryArgs {
    var path: String? = null
    var fileName: String? = null
    var mimeType: String? = null
    var album: String? = null
}

@InvokeArg
class SetWallpaperArgs {
    var path: String? = null
    var target: String? = null
}

/**
 * Thin Android layer. The Rust side has already validated and decoded the
 * image and written it to [SaveToGalleryArgs.path] / [SetWallpaperArgs.path];
 * this class only hands that file to MediaStore or WallpaperManager.
 *
 * Commands arrive on the main thread, so the I/O runs on a single background
 * thread. One thread also means two wallpaper calls can never overlap.
 */
@TauriPlugin
class ScenicPlugin(private val activity: Activity) : Plugin(activity) {

    private val worker: ExecutorService = Executors.newSingleThreadExecutor()

    @Command
    fun saveToGallery(invoke: Invoke) {
        val args = try {
            invoke.parseArgs(SaveToGalleryArgs::class.java)
        } catch (e: Exception) {
            invoke.reject("Invalid arguments: ${e.message}", "INVALID_ARGS")
            return
        }

        val path = args.path
        val fileName = args.fileName
        val mimeType = args.mimeType
        val album = args.album
        if (path.isNullOrBlank() || fileName.isNullOrBlank() || mimeType.isNullOrBlank() || album.isNullOrBlank()) {
            invoke.reject("path, fileName, mimeType and album are all required", "INVALID_ARGS")
            return
        }

        worker.execute {
            try {
                invoke.resolve(insertIntoPictures(File(path), fileName, mimeType, album))
            } catch (e: Exception) {
                Log.e(TAG, "saveToGallery failed", e)
                invoke.reject(e.message ?: "Could not save the image", "SAVE_FAILED", e)
            }
        }
    }

    @Command
    fun setWallpaper(invoke: Invoke) {
        val args = try {
            invoke.parseArgs(SetWallpaperArgs::class.java)
        } catch (e: Exception) {
            invoke.reject("Invalid arguments: ${e.message}", "INVALID_ARGS")
            return
        }

        val path = args.path
        if (path.isNullOrBlank()) {
            invoke.reject("path is required", "INVALID_ARGS")
            return
        }

        val target = args.target ?: "both"
        val flags = when (target) {
            "home" -> WallpaperManager.FLAG_SYSTEM
            "lock" -> WallpaperManager.FLAG_LOCK
            "both" -> WallpaperManager.FLAG_SYSTEM or WallpaperManager.FLAG_LOCK
            else -> {
                invoke.reject("Unknown target \"$target\"; use home, lock or both", "INVALID_ARGS")
                return
            }
        }

        val manager = WallpaperManager.getInstance(activity.applicationContext)
        if (!manager.isWallpaperSupported) {
            invoke.reject("This device does not support wallpapers", "UNSUPPORTED")
            return
        }
        if (!manager.isSetWallpaperAllowed) {
            invoke.reject("Changing the wallpaper is blocked on this device (device policy)", "NOT_ALLOWED")
            return
        }

        worker.execute {
            try {
                val file = requireReadable(File(path))
                // When only FLAG_SYSTEM is set and the lock screen was sharing
                // the old image, Android moves that old image to the lock
                // screen first, so "home" really leaves the lock screen alone.
                val id = FileInputStream(file).use { input ->
                    manager.setStream(input, null, true, flags)
                }
                if (id == 0) {
                    throw IllegalStateException("WallpaperManager refused the image")
                }
                val result = JSObject()
                result.put("target", target)
                result.put("wallpaperId", id)
                invoke.resolve(result)
            } catch (e: Exception) {
                Log.e(TAG, "setWallpaper failed", e)
                invoke.reject(e.message ?: "Could not set the wallpaper", "WALLPAPER_FAILED", e)
            }
        }
    }

    private fun insertIntoPictures(source: File, fileName: String, mimeType: String, album: String): JSObject {
        requireReadable(source)

        val resolver = activity.contentResolver
        val relativePath = "${Environment.DIRECTORY_PICTURES}/$album/"
        val collection = MediaStore.Images.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)

        val pending = ContentValues().apply {
            put(MediaStore.Images.Media.DISPLAY_NAME, fileName)
            put(MediaStore.Images.Media.MIME_TYPE, mimeType)
            put(MediaStore.Images.Media.RELATIVE_PATH, relativePath)
            // Hidden from other apps until the bytes are fully written.
            put(MediaStore.Images.Media.IS_PENDING, 1)
        }

        val uri: Uri = resolver.insert(collection, pending)
            ?: throw IllegalStateException("MediaStore did not create an entry")

        try {
            val out = resolver.openOutputStream(uri)
                ?: throw IllegalStateException("Could not open the gallery file for writing")
            out.use { o -> FileInputStream(source).use { it.copyTo(o) } }

            val done = ContentValues().apply { put(MediaStore.Images.Media.IS_PENDING, 0) }
            resolver.update(uri, done, null, null)
        } catch (e: Exception) {
            // Don't leave a half-written, invisible entry behind.
            try { resolver.delete(uri, null, null) } catch (_: Exception) {}
            throw e
        }

        val result = JSObject()
        result.put("uri", uri.toString())
        result.put("fileName", queryDisplayName(uri) ?: fileName)
        result.put("relativePath", relativePath)
        return result
    }

    private fun queryDisplayName(uri: Uri): String? {
        val projection = arrayOf(MediaStore.Images.Media.DISPLAY_NAME)
        return activity.contentResolver.query(uri, projection, null, null, null)?.use { cursor ->
            if (cursor.moveToFirst()) cursor.getString(0) else null
        }
    }

    private fun requireReadable(file: File): File {
        if (!file.isFile || !file.canRead()) {
            throw IllegalStateException("Image file is missing or unreadable: ${file.path}")
        }
        return file
    }
}
