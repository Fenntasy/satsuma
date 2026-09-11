module NowPlayingTest exposing (suite)

import Expect
import Json.Decode as Decode
import Json.Encode as Encode
import NowPlaying
import Test exposing (Test, describe, test)


suite : Test
suite =
    describe "NowPlaying"
        [ describe "decoder"
            [ test "reads what the panel shows" <|
                \_ ->
                    Decode.decodeString NowPlaying.decoder payload
                        |> Result.map
                            (\details ->
                                [ Maybe.withDefault "" details.title
                                , Maybe.withDefault "" details.artist
                                , Maybe.withDefault "" details.albumArtist
                                , Maybe.withDefault "" details.album
                                , Maybe.withDefault "" details.genre
                                , String.fromInt (Maybe.withDefault 0 details.year)
                                , String.fromInt (Maybe.withDefault 0 details.rating)
                                , Maybe.withDefault "" details.grouping
                                , details.coverKey
                                ]
                            )
                        |> Expect.equal
                            (Ok
                                [ "Orange Sun"
                                , "The Satsumas"
                                , "Various"
                                , "Citrus"
                                , "Indie"
                                , "1998"
                                , "4"
                                , "Chant / Loud / Happy"
                                , "411f5661726edd"
                                ]
                            )
            , test "a tag the file does not carry is nothing, not a failure" <|
                \_ ->
                    Decode.decodeString NowPlaying.decoder bare
                        |> Result.map (\details -> ( details.id, details.title, details.year ))
                        |> Expect.equal (Ok ( 7, Nothing, Nothing ))
            , test "a field that is not there at all is a failure, not a blank" <|
                \_ ->
                    -- The command and the decoder are on two sides of the
                    -- bridge: a renamed field has to be loud, or the panel
                    -- silently shows a record with no name.
                    Decode.decodeString NowPlaying.decoder withoutCoverKey
                        |> Expect.err
            ]
        , describe "trackChanged"
            [ test "nothing has changed before anything plays" <|
                \_ ->
                    NowPlaying.trackChanged Nothing NowPlaying.init
                        |> Expect.equal False
            , test "a track starting is a change" <|
                \_ ->
                    NowPlaying.trackChanged (Just 1) NowPlaying.init
                        |> Expect.equal True
            , test "the track we already asked about is not a change" <|
                \_ ->
                    -- The state arrives five times a second; asking again
                    -- for each of them until the answer lands would be a
                    -- command per state event.
                    NowPlaying.trackChanged (Just 1) (NowPlaying.loading 1)
                        |> Expect.equal False
            , test "the track being shown is not a change" <|
                \_ ->
                    showing 1
                        |> NowPlaying.trackChanged (Just 1)
                        |> Expect.equal False
            , test "the next track is a change" <|
                \_ ->
                    showing 1
                        |> NowPlaying.trackChanged (Just 2)
                        |> Expect.equal True
            , test "the music stopping is a change" <|
                \_ ->
                    showing 1
                        |> NowPlaying.trackChanged Nothing
                        |> Expect.equal True
            , test "a track we failed to read is not asked about again" <|
                \_ ->
                    -- Otherwise a track whose details will not decode is
                    -- asked for five times a second for as long as it
                    -- plays.
                    failed 1
                        |> NowPlaying.trackChanged (Just 1)
                        |> Expect.equal False
            , test "but the one after it is" <|
                \_ ->
                    failed 1
                        |> NowPlaying.trackChanged (Just 2)
                        |> Expect.equal True
            ]
        , describe "handleInvokeResult"
            [ test "leaves another panel's reply alone" <|
                \_ ->
                    NowPlaying.handleInvokeResult "list_playlists" (Ok Encode.null) NowPlaying.init
                        |> Expect.equal Nothing
            , test "a track that has left the library is not asked about again" <|
                \_ ->
                    -- Asserted against the id the player keeps reporting,
                    -- not against Nothing: the player holds its own queue
                    -- and goes on naming a track whose row a scan removed,
                    -- and `trackChanged Nothing` would hold however this
                    -- behaved.
                    NowPlaying.handleInvokeResult "track_details" (Ok Encode.null) (NowPlaying.loading 1)
                        |> Maybe.map (NowPlaying.trackChanged (Just 1))
                        |> Expect.equal (Just False)
            , test "but the track after it is" <|
                \_ ->
                    NowPlaying.handleInvokeResult "track_details" (Ok Encode.null) (NowPlaying.loading 1)
                        |> Maybe.map (NowPlaying.trackChanged (Just 2))
                        |> Expect.equal (Just True)
            , test "a refused command keeps which track it was about" <|
                \_ ->
                    NowPlaying.handleInvokeResult "track_details"
                        (Err "the library cannot be read")
                        (NowPlaying.loading 3)
                        |> Maybe.map (NowPlaying.trackChanged (Just 3))
                        |> Expect.equal (Just False)
            ]
        , describe "coverUrl"
            [ test "puts the key the backend issued after the platform's base" <|
                \_ ->
                    Decode.decodeString NowPlaying.decoder payload
                        |> Result.map (NowPlaying.coverUrl "satsuma-cover://localhost/")
                        |> Expect.equal (Ok "satsuma-cover://localhost/411f5661726edd")
            , test "works with the shape Windows uses" <|
                \_ ->
                    Decode.decodeString NowPlaying.decoder payload
                        |> Result.map (NowPlaying.coverUrl "http://satsuma-cover.localhost/")
                        |> Expect.equal (Ok "http://satsuma-cover.localhost/411f5661726edd")
            ]
        ]


{-| Reaches a model that is showing the track with this id.
-}
showing : Int -> NowPlaying.Model
showing id =
    NowPlaying.handleInvokeResult "track_details"
        (Ok (Encode.object (payloadFields id)))
        (NowPlaying.loading id)
        |> Maybe.withDefault NowPlaying.init


{-| Reaches a model that failed to read the track with this id.
-}
failed : Int -> NowPlaying.Model
failed id =
    NowPlaying.handleInvokeResult "track_details" (Err "nope") (NowPlaying.loading id)
        |> Maybe.withDefault NowPlaying.init


payloadFields : Int -> List ( String, Encode.Value )
payloadFields id =
    [ ( "id", Encode.int id )
    , ( "title", Encode.string "Orange Sun" )
    , ( "artist", Encode.string "The Satsumas" )
    , ( "album_artist", Encode.null )
    , ( "album", Encode.string "Citrus" )
    , ( "genre", Encode.null )
    , ( "year", Encode.null )
    , ( "rating", Encode.null )
    , ( "grouping", Encode.null )
    , ( "cover_key", Encode.string "411f5661726edd" )
    ]


payload : String
payload =
    """
    { "id": 1
    , "title": "Orange Sun"
    , "artist": "The Satsumas"
    , "album_artist": "Various"
    , "album": "Citrus"
    , "genre": "Indie"
    , "year": 1998
    , "rating": 4
    , "grouping": "Chant / Loud / Happy"
    , "cover_key": "411f5661726edd"
    }
    """


bare : String
bare =
    """
    { "id": 7
    , "title": null
    , "artist": null
    , "album_artist": null
    , "album": null
    , "genre": null
    , "year": null
    , "rating": null
    , "grouping": null
    , "cover_key": "54"
    }
    """


withoutCoverKey : String
withoutCoverKey =
    """
    { "id": 7
    , "title": null
    , "artist": null
    , "album_artist": null
    , "album": null
    , "genre": null
    , "year": null
    , "rating": null
    , "grouping": null
    }
    """
