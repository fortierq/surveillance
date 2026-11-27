package org.surveillance.app

import java.io.ByteArrayOutputStream
import java.io.IOException
import java.net.URI
import java.util.UUID
import javax.net.ssl.HttpsURLConnection
import org.json.JSONObject

data class Connection(val origin: String, val credential: String)

data class HostStatus(
    val armed: Boolean,
    val camera: String,
    val microphone: String,
    val pushConfigured: Boolean,
    val pushRegistered: Boolean,
    val liveVideo: Boolean,
)

data class Event(
    val id: String,
    val kind: String,
    val occurredAtMs: Long,
    val hasPhoto: Boolean,
    val hasAudio: Boolean,
)

data class History(val events: List<Event>, val nextBefore: Long?)

class HttpFailure(val status: Int) : IOException("Host returned HTTP $status")

fun hostOrigin(input: String): String {
    val invalid = "Enter an HTTPS host address, for example https://host.example.ts.net"
    val uri = runCatching { URI(input.trim()) }.getOrElse { throw IllegalArgumentException(invalid) }
    require(
        uri.scheme.equals("https", ignoreCase = true) && uri.host != null &&
            uri.userInfo == null && uri.rawPath in listOf("", "/") &&
            uri.rawQuery == null && uri.rawFragment == null &&
            (uri.port == -1 || uri.port in 1..65535),
    ) { invalid }
    return URI("https", null, uri.host, uri.port, null, null, null).toASCIIString()
}

fun eventId(value: String?): String? =
    value?.let { runCatching { UUID.fromString(it).toString() }.getOrNull() }?.takeIf { it == value }

class HostApi(private val origin: String, private val credential: String? = null) {
    constructor(connection: Connection) : this(connection.origin, connection.credential)

    fun pair(code: String): Connection {
        val response = request("POST", "/v1/pair", JSONObject().put("code", code))
        return Connection(origin, response.getString("credential"))
    }

    fun status(): HostStatus {
        val response = request("GET", "/v1/status")
        return HostStatus(
            response.getBoolean("armed"),
            response.getString("camera"),
            response.getString("microphone"),
            response.getBoolean("push_configured"),
            response.getBoolean("push_registered"),
            response.optBoolean("live_video"),
        )
    }

    fun setArmed(armed: Boolean) {
        request("PUT", "/v1/armed", JSONObject().put("armed", armed))
    }

    fun registerPush(token: String) {
        request("PUT", "/v1/push-token", JSONObject().put("token", token))
    }

    fun events(before: Long? = null): History {
        val response = request("GET", before?.let { "/v1/events?before=$it" } ?: "/v1/events")
        val array = response.getJSONArray("events")
        val events = List(array.length()) { index -> array.getJSONObject(index).asEvent() }
        return History(events, if (response.isNull("next_before")) null else response.getLong("next_before"))
    }

    fun event(id: String): Event {
        val validated = requireNotNull(eventId(id))
        return request("GET", "/v1/events/$validated").asEvent()
    }

    fun disconnect() {
        request("DELETE", "/v1/pair")
    }

    fun liveFrame(): ByteArray = fetch("GET", "/v1/live/frame", accept = "image/jpeg")
    fun photo(id: String): ByteArray = fetch("GET", "/v1/events/${requireNotNull(eventId(id))}/photo", accept = "image/jpeg")
    fun audio(id: String): ByteArray = fetch("GET", "/v1/events/${requireNotNull(eventId(id))}/audio", accept = "audio/wav")

    private fun request(method: String, path: String, body: JSONObject? = null): JSONObject {
        val data = fetch(method, path, body, "application/json")
        return if (data.isEmpty()) JSONObject() else JSONObject(data.toString(Charsets.UTF_8))
    }

    private fun fetch(method: String, path: String, body: JSONObject? = null, accept: String): ByteArray {
        val connection = URI(origin + path).toURL().openConnection() as HttpsURLConnection
        try {
            connection.requestMethod = method
            connection.instanceFollowRedirects = false
            connection.connectTimeout = 5_000
            connection.readTimeout = 5_000
            connection.setRequestProperty("Accept", accept)
            credential?.let { connection.setRequestProperty("Authorization", "Bearer $it") }
            if (body != null) {
                connection.doOutput = true
                connection.setRequestProperty("Content-Type", "application/json")
                connection.outputStream.use { it.write(body.toString().toByteArray(Charsets.UTF_8)) }
            }
            val status = connection.responseCode
            if (status !in 200..299) throw HttpFailure(status)
            if (status == 204) return byteArrayOf()
            return connection.inputStream.use { stream ->
                val output = ByteArrayOutputStream()
                val buffer = ByteArray(8192)
                while (true) {
                    val size = stream.read(buffer)
                    if (size == -1) break
                    if (output.size() + size > 1024 * 1024) throw IOException("Host response is too large")
                    output.write(buffer, 0, size)
                }
                output.toByteArray()
            }
        } finally {
            connection.disconnect()
        }
    }

    private fun JSONObject.asEvent() = Event(
        getString("id"), getString("kind"), getLong("occurred_at_ms"),
        optBoolean("has_photo"), optBoolean("has_audio"),
    )
}
