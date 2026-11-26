package ru.plastinka.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test

class ArtistLikesTest {
    private fun a(id: String) = ArtistCard(id, "n$id", null, 0)

    private fun ids(list: List<ArtistCard>) = list.map { it.id }

    @Test
    fun likeGoesFirstWithoutDuplicates() {
        val list = listOf(a("1"), a("2"), a("3"))
        assertEquals(listOf("9", "1", "2", "3"), ids(ArtistLikes.apply(list, a("9"), true)))
        assertEquals(listOf("2", "1", "3"), ids(ArtistLikes.apply(list, a("2"), true)))
    }

    @Test
    fun unlikeRemoves() {
        val list = listOf(a("1"), a("2"), a("3"))
        assertEquals(listOf("1", "3"), ids(ArtistLikes.apply(list, a("2"), false)))
        assertEquals(listOf("1", "2", "3"), ids(ArtistLikes.apply(list, a("7"), false)))
    }

    @Test
    fun rollbackOfLikeRemovesIt() {
        val before = listOf(a("1"), a("2"))
        val current = ArtistLikes.apply(before, a("9"), true)
        assertEquals(listOf("1", "2"), ids(ArtistLikes.rollback(current, before, a("9"), false)))
    }

    @Test
    fun rollbackOfUnlikeRestoresPlace() {
        val before = listOf(a("1"), a("2"), a("3"))
        val current = ArtistLikes.apply(before, a("2"), false)
        assertEquals(listOf("1", "2", "3"), ids(ArtistLikes.rollback(current, before, a("2"), true)))
        // Список за это время стал короче: место не выходит за границы.
        assertEquals(listOf("5", "3"), ids(ArtistLikes.rollback(listOf(a("5")), listOf(a("1"), a("2"), a("3")), a("3"), true)))
    }

    @Test
    fun requestShape() {
        val add = YandexApi.artistLikeRequest("tok", 42L, "123", true)
        assertNotNull(add)
        assertEquals("https://api.music.yandex.net/users/42/likes/artists/add-multiple", add?.url?.toString())
        assertEquals("POST", add?.method)
        assertEquals("OAuth tok", add?.header("Authorization"))
        val remove = YandexApi.artistLikeRequest("tok", 42L, "123", false)
        assertEquals("https://api.music.yandex.net/users/42/likes/artists/remove", remove?.url?.toString())
    }

    @Test
    fun nonNumericIdIsNotSent() {
        assertNull(YandexApi.artistLikeRequest("tok", 42L, "12a", true))
        assertNull(YandexApi.artistLikeRequest("tok", 42L, "", true))
        assertNull(YandexApi.artistLikeRequest("tok", 42L, "1/../2", false))
    }
}
