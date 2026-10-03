package io.github.tymonoman.extraspace

/** Allowed methods, independent of the temporary link used for discovery. */
enum class TransportMode(val wireName: String) {
    AUTO("auto"), ADB("adb"), ACCESSORY("accessory");

    fun allows(method: TransportMode): Boolean = method != AUTO && (this == AUTO || this == method)

    fun select(peer: TransportMode): TransportMode = when {
        this == AUTO && peer == AUTO -> ADB
        this == AUTO -> peer
        peer == AUTO || this == peer -> this
        else -> throw ProtocolException(INCOMPATIBLE)
    }

    /** ADB can discover an accessory-only selection; accessory cannot switch to ADB. */
    fun selectForLink(peer: TransportMode, link: TransportMode): TransportMode {
        val selected = select(peer)
        if (link == ACCESSORY) {
            if (!allows(link) || !peer.allows(link)) {
                throw ProtocolException("Incompatible connection methods: the shared method is ADB, but the device is in USB accessory mode. Reconnect with USB debugging enabled.")
            }
            return ACCESSORY
        }
        require(link == ADB) { "Discovery requires a concrete connection method" }
        return selected
    }

    companion object {
        const val INCOMPATIBLE = "Incompatible connection methods: one app allows only ADB and the other only USB accessory. Select a shared method or Automatic in both apps."
        fun parse(value: String): TransportMode = entries.firstOrNull { it.wireName == value }
            ?: throw ProtocolException("Unknown connection method: $value")
    }
}
