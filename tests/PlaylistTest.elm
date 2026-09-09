module PlaylistTest exposing (suite)

import Expect
import Json.Decode as Decode
import Playlist exposing (Column(..))
import Test exposing (Test, describe, test)
import Tree


suite : Test
suite =
    describe "Playlist"
        [ describe "columns"
            [ test "are the ones the requirements ask for, in order" <|
                \_ ->
                    List.map Playlist.columnLabel Playlist.columns
                        |> Expect.equal
                            [ "Genre"
                            , "Artist"
                            , "Track"
                            , "Title"
                            , "Album"
                            , "Duration"
                            , "Rating"
                            , "Grouping"
                            ]
            ]
        , describe "cellText"
            [ test "reads the tag of the column" <|
                \_ ->
                    [ Genre, Artist, Track, Title, Album, Grouping ]
                        |> List.map (\column -> Playlist.cellText column song)
                        |> Expect.equal
                            [ "Indie", "Alpha", "3", "One", "First", "Chant / Loud / Happy" ]
            , test "shows a duration as minutes and seconds" <|
                \_ -> Playlist.cellText Duration song |> Expect.equal "3:05"
            , test "leaves a missing tag empty rather than saying Unknown" <|
                \_ ->
                    [ Genre, Artist, Track, Title, Album, Rating, Grouping ]
                        |> List.map (\column -> Playlist.cellText column bare)
                        |> Expect.equal [ "", "", "", "", "", "", "" ]
            ]
        , describe "sorting"
            [ test "keeps the order of the playlist without a sort" <|
                \_ ->
                    Playlist.sortBy Nothing library
                        |> titles
                        |> Expect.equal [ "Beta song", "alpha song", "Gamma song" ]
            , test "orders by a text column, ignoring case" <|
                \_ ->
                    Playlist.sortBy (Just { column = Title, ascending = True }) library
                        |> titles
                        |> Expect.equal [ "alpha song", "Beta song", "Gamma song" ]
            , test "turns the order around" <|
                \_ ->
                    Playlist.sortBy (Just { column = Title, ascending = False }) library
                        |> titles
                        |> Expect.equal [ "Gamma song", "Beta song", "alpha song" ]
            , test "orders a track number as a number, not as text" <|
                \_ ->
                    -- As text, "10" would come before "9".
                    Playlist.sortBy (Just { column = Track, ascending = True }) numbered
                        |> List.filterMap .trackNumber
                        |> Expect.equal [ 2, 9, 10 ]
            , test "orders by duration" <|
                \_ ->
                    Playlist.sortBy (Just { column = Duration, ascending = True }) library
                        |> List.map .durationMs
                        |> Expect.equal [ 60000, 120000, 185000 ]
            , test "orders by rating, unrated first" <|
                \_ ->
                    Playlist.sortBy (Just { column = Rating, ascending = True }) library
                        |> List.map (.rating >> Maybe.withDefault 0)
                        |> Expect.equal [ 0, 2, 5 ]
            ]
        , describe "toggleSort"
            [ test "orders by a new column, ascending" <|
                \_ ->
                    Playlist.toggleSort Title Nothing
                        |> Expect.equal (Just { column = Title, ascending = True })
            , test "turns the order around on the column in use" <|
                \_ ->
                    Playlist.toggleSort Title (Just { column = Title, ascending = True })
                        |> Expect.equal (Just { column = Title, ascending = False })
            , test "a third click gives the playlist its own order back" <|
                \_ ->
                    Playlist.toggleSort Title (Just { column = Title, ascending = False })
                        |> Expect.equal Nothing
            , test "another column starts ascending again" <|
                \_ ->
                    Playlist.toggleSort Album (Just { column = Title, ascending = False })
                        |> Expect.equal (Just { column = Album, ascending = True })
            ]
        , describe "footerText"
            [ test "counts the tracks and adds up their time" <|
                \_ ->
                    Playlist.footerText library
                        |> Expect.equal "3 tracks [6:05]"
            , test "uses the singular for one track" <|
                \_ -> Playlist.footerText [ song ] |> Expect.equal "1 track [3:05]"
            , test "an empty playlist says so" <|
                \_ -> Playlist.footerText [] |> Expect.equal "0 tracks [0:00]"
            , test "goes past an hour" <|
                \_ ->
                    Playlist.footerText (List.repeat 20 song)
                        |> Expect.equal "20 tracks [1:01:40]"
            ]
        , describe "decoder"
            [ test "reads a playlist and its tracks" <|
                \_ ->
                    Decode.decodeString Playlist.decoder playlistJson
                        |> Result.map (\playlist -> ( playlist.name, List.length playlist.tracks ))
                        |> Expect.equal (Ok ( "Favourites", 1 ))
            ]
        ]


song : Tree.Row
song =
    { id = 1
    , genre = Just "Indie"
    , artist = Just "Alpha"
    , album = Just "First"
    , title = Just "One"
    , trackNumber = Just 3
    , discNumber = Just 1
    , rating = Just 4
    , grouping = Just "Chant / Loud / Happy"
    , durationMs = 185000
    }


bare : Tree.Row
bare =
    { id = 2
    , genre = Nothing
    , artist = Nothing
    , album = Nothing
    , title = Nothing
    , trackNumber = Nothing
    , discNumber = Nothing
    , rating = Nothing
    , grouping = Nothing
    , durationMs = 0
    }


library : List Tree.Row
library =
    [ { song | id = 1, title = Just "Beta song", durationMs = 120000, rating = Just 2 }
    , { song | id = 2, title = Just "alpha song", durationMs = 185000, rating = Just 5 }
    , { song | id = 3, title = Just "Gamma song", durationMs = 60000, rating = Nothing }
    ]


numbered : List Tree.Row
numbered =
    [ { song | id = 1, trackNumber = Just 10 }
    , { song | id = 2, trackNumber = Just 2 }
    , { song | id = 3, trackNumber = Just 9 }
    ]


titles : List Tree.Row -> List String
titles =
    List.filterMap .title


playlistJson : String
playlistJson =
    """
    { "id": 7
    , "name": "Favourites"
    , "tracks":
        [ { "id": 1
          , "genre": "Indie"
          , "artist": "Alpha"
          , "album": "First"
          , "title": "One"
          , "track_number": 3
          , "disc_number": 1
          , "rating": 4
          , "grouping": "Chant / Loud / Happy"
          , "duration_ms": 185000
          }
        ]
    }
    """
