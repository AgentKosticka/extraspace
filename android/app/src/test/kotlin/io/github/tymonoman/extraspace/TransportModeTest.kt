package io.github.tymonoman.extraspace

import org.junit.Assert.*
import org.junit.Test

class TransportModeTest {
    @Test fun sharedSelectorMatrix() {
        val rows = javaClass.getResourceAsStream("/transport-selection.tsv")!!
            .bufferedReader().use { it.readLines() }.filter { !it.startsWith("#") && it.isNotBlank() }
        assertEquals(9, rows.size)
        for (row in rows) {
            val (host, device, expected) = row.split('\t')
            if (expected == "incompatible") {
                try {
                    TransportMode.parse(host).select(TransportMode.parse(device))
                    fail(row)
                } catch (error: ProtocolException) {
                    assertEquals(TransportMode.INCOMPATIBLE, error.message)
                }
            } else {
                val selected = TransportMode.parse(host).select(TransportMode.parse(device))
                assertEquals(row, expected, selected.wireName)
                assertTrue(TransportMode.parse(host).allows(selected))
                assertTrue(TransportMode.parse(device).allows(selected))
            }
        }
    }

    @Test fun unknownModeCannotSilentlyAllowBoth() {
        try { TransportMode.parse("bogus"); fail("Invalid mode accepted") }
        catch (_: ProtocolException) { }
        assertFalse(TransportMode.ADB.allows(TransportMode.ACCESSORY))
        assertFalse(TransportMode.ACCESSORY.allows(TransportMode.ADB))
        assertFalse(TransportMode.AUTO.allows(TransportMode.AUTO))
    }

    @Test fun accessoryFallbackNeverBypassesAnAdbOnlySelector() {
        for (host in TransportMode.entries) for (device in TransportMode.entries) {
            if (host.allows(TransportMode.ACCESSORY) && device.allows(TransportMode.ACCESSORY)) {
                assertEquals(TransportMode.ACCESSORY, host.selectForLink(device, TransportMode.ACCESSORY))
            } else {
                try {
                    host.selectForLink(device, TransportMode.ACCESSORY)
                    fail("Accessory bypassed $host / $device")
                } catch (error: ProtocolException) {
                    assertTrue(error.message!!.startsWith("Incompatible connection methods:"))
                }
            }
        }
        assertEquals(TransportMode.ACCESSORY,
            TransportMode.AUTO.selectForLink(TransportMode.ACCESSORY, TransportMode.ADB))
        assertEquals(TransportMode.ACCESSORY,
            TransportMode.ACCESSORY.selectForLink(TransportMode.AUTO, TransportMode.ADB))
    }
}
