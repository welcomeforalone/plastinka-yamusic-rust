package ru.plastinka.ui

import ru.plastinka.data.ArtistRef
import ru.plastinka.data.Track
import ru.plastinka.data.YandexApi

/**
 * Запрос «открыть маршрут» из полноэкранного плеера: адресован вкладке [tab]
 * (1 — «Поиск», 2 — «Коллекция»). [seq] отличает два одинаковых запроса подряд.
 */
data class RouteRequest(val route: CollectionRoute, val tab: Int, val seq: Int)

/** Вкладка для перехода: «Поиск» и «Коллекция» остаются как есть, остальные уводят в «Коллекцию». */
internal fun tabForRouteOpen(current: Int): Int = if (current == 1 || current == 2) current else 2

/** Маршрут уже открыт: тот же альбом или тот же исполнитель по id. */
internal fun sameRouteTarget(a: CollectionRoute?, b: CollectionRoute): Boolean = when {
    a is CollectionRoute.Album && b is CollectionRoute.Album -> a.id == b.id
    a is CollectionRoute.Artist && b is CollectionRoute.Artist -> a.id == b.id
    else -> false
}

/** Кладёт маршрут на стек, если наверху уже не тот же альбом или исполнитель. */
internal fun pushRouteUnique(stack: List<CollectionRoute>, route: CollectionRoute): List<CollectionRoute> =
    if (sameRouteTarget(stack.lastOrNull(), route)) stack else stack + route

/** Альбом трека для перехода; нет настоящего номера — `null` (название не нажимается). */
internal fun albumRouteOf(track: Track): CollectionRoute? {
    val id = track.albumId?.takeIf(YandexApi::isNumericId) ?: return null
    // Обложка трека — та же, что у альбома: первый кадр экрана альбома уже с ней.
    return CollectionRoute.Album(id, track.albumTitle.orEmpty(), track.coverUri)
}

/** Исполнители с настоящим номером: только их страницы можно открыть. */
internal fun linkableArtists(track: Track): List<ArtistRef> =
    track.artists.filter { it.id?.let(YandexApi::isNumericId) == true }

internal fun artistRouteOf(artist: ArtistRef): CollectionRoute? =
    artist.id?.takeIf(YandexApi::isNumericId)?.let { CollectionRoute.Artist(it, artist.name, null) }
