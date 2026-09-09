module Playlist exposing
    ( Column(..)
    , Playlist
    , Sort
    , cellText
    , columnLabel
    , columns
    , decoder
    , defaultWidth
    , footerText
    , sortBy
    , toggleSort
    )

{-| What a playlist holds and how its table is ordered.

Pure logic only: the panel that draws it lives in `Main`, and everything
decided here (which column, which direction, what a cell says) is tested
directly.

-}

import Json.Decode as Decode exposing (Decoder)
import Tree


type alias Playlist =
    { id : Int
    , name : String
    , tracks : List Tree.Row
    }


{-| The columns of the table, in the order they are shown.
-}
type Column
    = Genre
    | Artist
    | Track
    | Title
    | Album
    | Duration
    | Rating
    | Grouping


columns : List Column
columns =
    [ Genre, Artist, Track, Title, Album, Duration, Rating, Grouping ]


columnLabel : Column -> String
columnLabel column =
    case column of
        Genre ->
            "Genre"

        Artist ->
            "Artist"

        Track ->
            "Track"

        Title ->
            "Title"

        Album ->
            "Album"

        Duration ->
            "Duration"

        Rating ->
            "Rating"

        Grouping ->
            "Grouping"


{-| How wide a column starts out, in pixels.
-}
defaultWidth : Column -> Int
defaultWidth column =
    case column of
        Genre ->
            110

        Artist ->
            160

        Track ->
            60

        Title ->
            240

        Album ->
            180

        Duration ->
            80

        Rating ->
            90

        Grouping ->
            150


{-| Which column the table is ordered by, and which way.
-}
type alias Sort =
    { column : Column
    , ascending : Bool
    }


{-| Clicking a column heading orders by it, and clicking the one already in
use turns the order around.
-}
toggleSort : Column -> Maybe Sort -> Maybe Sort
toggleSort column current =
    case current of
        Just sort ->
            if sort.column == column then
                if sort.ascending then
                    Just { sort | ascending = False }

                else
                    -- A third click leaves the order the playlist has.
                    Nothing

            else
                Just { column = column, ascending = True }

        Nothing ->
            Just { column = column, ascending = True }


{-| Orders the tracks. Without a sort they keep the order of the playlist.
-}
sortBy : Maybe Sort -> List Tree.Row -> List Tree.Row
sortBy sort tracks =
    case sort of
        Nothing ->
            tracks

        Just { column, ascending } ->
            let
                ordered : List Tree.Row
                ordered =
                    case column of
                        Track ->
                            List.sortBy (\track -> Maybe.withDefault 0 track.trackNumber) tracks

                        Duration ->
                            List.sortBy .durationMs tracks

                        Rating ->
                            List.sortBy (\track -> Maybe.withDefault 0 track.rating) tracks

                        _ ->
                            List.sortBy (cellText column >> String.toLower) tracks
            in
            if ascending then
                ordered

            else
                List.reverse ordered


{-| What a cell says. An empty string where the tag is missing: the table
is dense enough without "Unknown" repeated across it.
-}
cellText : Column -> Tree.Row -> String
cellText column track =
    case column of
        Genre ->
            Maybe.withDefault "" track.genre

        Artist ->
            Maybe.withDefault "" track.artist

        Track ->
            track.trackNumber |> Maybe.map String.fromInt |> Maybe.withDefault ""

        Title ->
            Maybe.withDefault "" track.title

        Album ->
            Maybe.withDefault "" track.album

        Duration ->
            formatDuration track.durationMs

        Rating ->
            track.rating |> Maybe.map String.fromInt |> Maybe.withDefault ""

        Grouping ->
            Maybe.withDefault "" track.grouping


{-| The line under the table: how many tracks, and how long they run.
-}
footerText : List Tree.Row -> String
footerText tracks =
    let
        count : Int
        count =
            List.length tracks

        tracksText : String
        tracksText =
            if count == 1 then
                "1 track"

            else
                String.fromInt count ++ " tracks"
    in
    tracksText ++ " [" ++ formatDuration (List.sum (List.map .durationMs tracks)) ++ "]"


formatDuration : Int -> String
formatDuration ms =
    let
        totalSeconds : Int
        totalSeconds =
            ms // 1000

        hours : Int
        hours =
            totalSeconds // 3600

        minutes : Int
        minutes =
            modBy 60 (totalSeconds // 60)

        seconds : Int
        seconds =
            modBy 60 totalSeconds

        pad : Int -> String
        pad n =
            String.padLeft 2 '0' (String.fromInt n)
    in
    if hours > 0 then
        String.fromInt hours ++ ":" ++ pad minutes ++ ":" ++ pad seconds

    else
        String.fromInt minutes ++ ":" ++ pad seconds


decoder : Decoder Playlist
decoder =
    Decode.map3 Playlist
        (Decode.field "id" Decode.int)
        (Decode.field "name" Decode.string)
        (Decode.field "tracks" (Decode.list Tree.rowDecoder))
