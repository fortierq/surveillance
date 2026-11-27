package org.surveillance.app

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.media.AudioAttributes
import android.media.MediaDataSource
import android.media.MediaPlayer
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import java.io.IOException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

@Composable
fun LiveView(api: HostApi, onClose: () -> Unit) {
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    var frame by remember { mutableStateOf<Bitmap?>(null) }
    var message by remember { mutableStateOf("Connecting to the camera…") }
    LaunchedEffect(api, lifecycle) {
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            try {
                while (isActive) {
                    try {
                        frame = withContext(Dispatchers.IO) { decodeJpeg(api.liveFrame()) }
                    } catch (error: IOException) {
                        frame = null
                        message = when ((error as? HttpFailure)?.status) {
                            401 -> "Pair this phone again to view the camera."
                            404 -> "Waiting for the armed camera…"
                            else -> "Cannot reach the camera. Check the host connection."
                        }
                        delay(1000)
                    }
                    delay(250)
                }
            } finally {
                frame = null
            }
        }
    }
    AlertDialog(
        onDismissRequest = onClose,
        title = { Text("Live view") },
        text = {
            val latest = frame
            if (latest == null) Text(message) else CameraImage(latest, "Live camera")
        },
        confirmButton = { TextButton(onClick = onClose) { Text("Close") } },
    )
}

@Composable
fun EventMediaView(api: HostApi, event: Event) {
    val scope = rememberCoroutineScope()
    var photo by remember(event.id) { mutableStateOf<Bitmap?>(null) }
    var photoMessage by remember(event.id) { mutableStateOf("Loading photo…") }
    var audioMessage by remember(event.id) { mutableStateOf<String?>(null) }
    var player by remember(event.id) { mutableStateOf<MediaPlayer?>(null) }
    var audioJob by remember(event.id) { mutableStateOf<Job?>(null) }

    LaunchedEffect(api, event.id, event.hasPhoto) {
        if (event.hasPhoto) {
            try {
                photo = withContext(Dispatchers.IO) { decodeJpeg(api.photo(event.id)) }
            } catch (error: IOException) {
                photoMessage = if ((error as? HttpFailure)?.status == 404) "This photo has expired." else "Could not load the event photo."
            }
        }
    }

    fun stopAudio() {
        audioJob?.cancel()
        audioJob = null
        player?.release()
        player = null
    }

    DisposableEffect(event.id) { onDispose { stopAudio() } }
    LifecycleEventEffect(Lifecycle.Event.ON_STOP) { stopAudio() }

    Column {
        if (event.hasPhoto) {
            val image = photo
            if (image == null) Text(photoMessage) else CameraImage(image, "Event photo")
        }
        if (event.hasAudio) {
            TextButton(onClick = {
                if (player != null || audioJob?.isActive == true) {
                    stopAudio()
                } else {
                    audioMessage = null
                    audioJob = scope.launch {
                        try {
                            val wav = withContext(Dispatchers.IO) { api.audio(event.id) }
                            val playback = MediaPlayer()
                            player = playback
                            playback.setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).build())
                            playback.setDataSource(WavData(wav))
                            playback.setOnPreparedListener { if (player === it) it.start() }
                            playback.setOnCompletionListener { if (player === it) stopAudio() }
                            playback.setOnErrorListener { failed, _, _ ->
                                if (player === failed) {
                                    stopAudio()
                                    audioMessage = "Could not play the audio clip."
                                }
                                true
                            }
                            playback.prepareAsync()
                        } catch (error: IOException) {
                            stopAudio()
                            audioMessage = if ((error as? HttpFailure)?.status == 404) "This audio clip has expired." else "Could not load the audio clip."
                        } catch (_: IllegalArgumentException) {
                            stopAudio()
                            audioMessage = "Could not play the audio clip."
                        }
                    }
                }
            }) {
                Text(if (player != null || audioJob?.isActive == true) "Stop audio" else "Play audio")
            }
            audioMessage?.let { Text(it) }
        }
        if (!event.hasPhoto && !event.hasAudio) Text("No media was saved for this event.")
    }
}

@Composable
private fun CameraImage(bitmap: Bitmap, description: String) {
    Image(
        bitmap = remember(bitmap) { bitmap.asImageBitmap() },
        contentDescription = description,
        modifier = Modifier.fillMaxWidth().aspectRatio(bitmap.width.toFloat() / bitmap.height),
    )
}

private fun decodeJpeg(data: ByteArray): Bitmap =
    BitmapFactory.decodeByteArray(data, 0, data.size) ?: throw IOException("Invalid JPEG frame")

private class WavData(private val bytes: ByteArray) : MediaDataSource() {
    override fun getSize(): Long = bytes.size.toLong()

    override fun readAt(position: Long, buffer: ByteArray, offset: Int, size: Int): Int {
        if (size == 0) return 0
        if (position < 0 || position >= bytes.size) return -1
        val start = position.toInt()
        val length = minOf(size, bytes.size - start)
        bytes.copyInto(buffer, offset, start, start + length)
        return length
    }

    override fun close() = Unit
}
