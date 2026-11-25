package ru.plastinka.sync

import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import ru.plastinka.data.Loudness

class MessagesTest {
    private fun parse(json: String) = SyncMessages.parse(json.toByteArray(Charsets.UTF_8))

    private fun roundTrip(m: SyncMessage): SyncMessage? = SyncMessages.parse(SyncMessages.encode(m))

    @Test
    fun simpleMessagesRoundTrip() {
        for (m in listOf(
            SyncMessage.Ping(17, 123456789),
            SyncMessage.Pong(17, 123456789, 987654321),
            SyncMessage.Tick(42, 62240, 123457795, false),
            SyncMessage.Tick(42, 0, 5, true),
            SyncMessage.Command("dislike"),
            SyncMessage.Liked("12345", true),
            SyncMessage.Bye("user"),
            SyncMessage.Confirm,
        )) {
            assertEquals(m, roundTrip(m))
        }
        val auth = roundTrip(SyncMessage.Auth(ByteArray(32) { it.toByte() }))
        assertContentEquals(ByteArray(32) { it.toByte() }, (auth as SyncMessage.Auth).mac)
    }

    @Test
    fun fieldNamesAreAsInSpec() {
        assertEquals(
            """{"t":"ping","n":17,"t0":123456789}""",
            SyncMessages.encode(SyncMessage.Ping(17, 123456789)).toString(Charsets.UTF_8),
        )
        assertEquals(
            """{"t":"tick","rev":42,"positionMs":62240,"at":123457795,"buffering":false}""",
            SyncMessages.encode(SyncMessage.Tick(42, 62240, 123457795, false)).toString(Charsets.UTF_8),
        )
        val state = SyncMessages.encode(SyncMessage.State(sampleState())).toString(Charsets.UTF_8)
        for (k in listOf(
            "\"rev\"", "\"by\"", "\"owner\"", "\"track\"", "\"playing\"", "\"positionMs\"", "\"at\"", "\"next\"",
            "\"shuffle\"", "\"repeat\"", "\"wave\"", "\"initial\"", "\"join\"", "\"albumId\"", "\"albumTitle\"",
            "\"durationMs\"", "\"coverUri\"", "\"available\"", "\"warning\":\"explicit\"", "\"truePeak\"",
        )) {
            assert(state.contains(k)) { "нет $k в $state" }
        }
    }

    @Test
    fun stateRoundTripWithTracks() {
        val s = sampleState(initial = true, join = true)
            .copy(next = sampleTrack("777"), shuffle = true, repeat = "all", wave = true)
        assertEquals(SyncMessage.State(s), roundTrip(SyncMessage.State(s)))
        val empty = sampleState(track = null, playing = false)
        assertEquals(SyncMessage.State(empty), roundTrip(SyncMessage.State(empty)))
    }

    @Test
    fun cleanWarningAndNullLoudness() {
        val t = sampleTrack().copy(explicit = false, clean = true, loudness = null, version = null)
        assertEquals(t, SyncMessages.parseTrack(SyncMessages.trackJson(t)))
    }

    @Test
    fun unknownTypeAndGarbageAreSkipped() {
        assertNull(parse("""{"t":"future","x":1}"""))
        assertNull(parse("""{"x":1}"""))
        assertNull(parse("не json"))
        assertNull(parse("[1,2]"))
        assertNull(parse("""{"t":"ping","n":"x","t0":1}"""))
        assertNull(parse("""{"t":"command","action":"explode"}"""))
        assertNull(parse("""{"t":"liked","track":"a b","liked":true}"""))
        assertNull(parse("""{"t":"auth","mac":"AAAA"}"""))
    }

    @Test
    fun extraFieldsAreIgnored() {
        assertEquals(SyncMessage.Ping(1, 2), parse("""{"t":"ping","n":1,"t0":2,"extra":[1,2,3]}"""))
    }

    private fun stateWith(track: String) =
        """{"t":"state","rev":1,"by":"a","owner":"a","track":$track,"playing":true,"positionMs":0,"at":0,"next":null,""" +
            """"shuffle":false,"repeat":"off","wave":false,"initial":false,"join":false}"""

    private fun trackJson(
        id: String = "\"12345\"",
        albumId: String = "\"678\"",
        title: String = "\"Т\"",
        artists: String = """[{"id":"9","name":"И"}]""",
        duration: String = "215000",
        cover: String = "\"avatars.yandex.net/x/%%\"",
        warning: String = "null",
        loudness: String = "null",
    ) = """{"id":$id,"albumId":$albumId,"albumTitle":null,"title":$title,"version":null,"artists":$artists,""" +
        """"durationMs":$duration,"coverUri":$cover,"available":true,"warning":$warning,"loudness":$loudness}"""

    private fun stateTrack(json: String) = (parse(stateWith(json)) as? SyncMessage.State)?.state?.track

    @Test
    fun validTrackPasses() {
        val t = assertNotNull(
            stateTrack(trackJson(warning = "\"clean\"", loudness = """{"integrated":-9.1,"truePeak":0.3}""")),
        )
        assertEquals("12345", t.id)
        assertEquals(Loudness(-9.1, 0.3), t.loudness)
        assertEquals(true, t.clean)
    }

    @Test
    fun trackChecks() {
        assertNull(parse(stateWith(trackJson(id = "\"a b\""))))
        assertNull(parse(stateWith(trackJson(id = "\"\""))))
        assertNull(parse(stateWith(trackJson(id = "\"${"x".repeat(65)}\""))))
        assertNull(parse(stateWith(trackJson(id = "12"))))
        assertNull(parse(stateWith(trackJson(albumId = "\"12a\""))))
        assertNull(parse(stateWith(trackJson(albumId = "\"${"1".repeat(21)}\""))))
        assertNull(parse(stateWith(trackJson(title = "\"${"я".repeat(513)}\""))))
        assertNotNull(parse(stateWith(trackJson(title = "\"${"я".repeat(512)}\""))))
        val many = (1..21).joinToString(",", "[", "]") { """{"id":"$it","name":"n"}""" }
        assertNull(parse(stateWith(trackJson(artists = many))))
        assertNull(parse(stateWith(trackJson(artists = """[{"id":"x1","name":"n"}]"""))))
        assertNull(parse(stateWith(trackJson(duration = "86400001"))))
        assertNull(parse(stateWith(trackJson(duration = "-1"))))
        assertNotNull(parse(stateWith(trackJson(duration = "86400000"))))
        assertNull(parse(stateWith(trackJson(warning = "\"bad\""))))
        assertNull(parse(stateWith(trackJson(loudness = """{"integrated":"x","truePeak":0.3}"""))))
        assertNull(parse(stateWith(trackJson(loudness = """{"integrated":1}"""))))
    }

    @Test
    fun coverFromForeignHostBecomesNullButStateStays() {
        assertNull(stateTrack(trackJson(cover = "\"evil.example.com/x/%%\""))?.sourceCoverUri)
        assertNull(stateTrack(trackJson(cover = "\"avatars.yandex.net.evil.com/x/%%\""))?.sourceCoverUri)
        assertNull(stateTrack(trackJson(cover = "\"https://avatars.yandex.net/x/%%\""))?.sourceCoverUri)
        assertNull(stateTrack(trackJson(cover = "\"avatars.yandex.net/x/no-template\""))?.sourceCoverUri)
        assertEquals(
            "avatars.mds.yandex.net/x/%%",
            stateTrack(trackJson(cover = "\"avatars.mds.yandex.net/x/%%\""))?.sourceCoverUri,
        )
        assertEquals(
            "music-avatars.music.yandex.net/x/%%",
            stateTrack(trackJson(cover = "\"music-avatars.music.yandex.net/x/%%\""))?.sourceCoverUri,
        )
        assertEquals("avatars.yandex.net/x/%%", stateTrack(trackJson())?.sourceCoverUri)
        assertNotNull(stateTrack(trackJson(cover = "null")))
    }

    @Test
    fun coverMustBeVisibleAscii() {
        assertNull(stateTrack(trackJson(cover = "\"avatars.yandex.net/x y/%%\""))?.sourceCoverUri)
        assertNull(stateTrack(trackJson(cover = "\"avatars.yandex.net/ы/%%\""))?.sourceCoverUri)
        assertNotNull(stateTrack(trackJson(cover = "\"avatars.yandex.net/x y/%%\"")))
    }

    @Test
    fun availableAndArtistsAreOptional() {
        val t = assertNotNull(stateTrack("""{"id":"1"}"""))
        assertEquals(false, t.available)
        assertEquals(emptyList(), t.sourceArtists)
    }

    @Test
    fun lengthsAreInCodePoints() {
        // 512 символов вне BMP — 1024 единицы UTF-16, но 512 кодовых точек: допустимо.
        val note = String(Character.toChars(0x1F3B5))
        assertNotNull(parse(stateWith(trackJson(title = "\"${note.repeat(512)}\""))))
        assertNull(parse(stateWith(trackJson(title = "\"${note.repeat(513)}\""))))
    }

    @Test
    fun stateWithBadFieldsIsSkipped() {
        assertNull(parse(stateWith("null").replace("\"repeat\":\"off\"", "\"repeat\":\"x\"")))
        assertNull(parse(stateWith("null").replace("\"rev\":1", "\"rev\":-1")))
        assertNull(parse(stateWith("null").replace("\"by\":\"a\"", "\"by\":5")))
        assertNotNull(parse(stateWith("null")))
    }
}
