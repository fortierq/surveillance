package org.surveillance.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Test

class HostAddressTest {
    @Test
    fun acceptsHttpsOrigins() {
        assertEquals("https://host.example.ts.net", hostOrigin("  HTTPS://host.example.ts.net/  "))
        assertEquals("https://[::1]:8765", hostOrigin("https://[::1]:8765"))
    }

    @Test
    fun rejectsCleartextAndUrlsThatAreNotOrigins() {
        for (address in listOf("http://host", "https://user@host", "https://host/api", "https://host?query", "https://host:99999")) {
            assertThrows(IllegalArgumentException::class.java) { hostOrigin(address) }
        }
    }

    @Test
    fun acceptsOnlyCanonicalEventIdentifiers() {
        val id = "efb03720-ba8d-459b-84e3-3289433d99c7"
        assertEquals(id, eventId(id))
        assertNull(eventId("1-1-1-1-1"))
        assertNull(eventId("../status"))
    }
}
