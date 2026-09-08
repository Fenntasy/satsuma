module PlayerTest exposing (suite)

import Expect
import Json.Decode as Decode
import Json.Encode as Encode
import Player exposing (Repeat(..), Status(..))
import Test exposing (Test, describe, test)


suite : Test
suite =
    describe "Player"
        [ describe "formatPosition"
            [ test "minutes and seconds" <|
                \_ -> Player.formatPosition 65000 |> Expect.equal "1:05"
            , test "zero" <|
                \_ -> Player.formatPosition 0 |> Expect.equal "0:00"
            , test "past an hour" <|
                \_ -> Player.formatPosition 3723000 |> Expect.equal "1:02:03"
            , test "rounds down to the second" <|
                \_ -> Player.formatPosition 1999 |> Expect.equal "0:01"
            ]
        , describe "handleEvent"
            [ test "a playing state is mirrored" <|
                \_ ->
                    state playingJson
                        |> Expect.equal
                            (Ok
                                (Just
                                    { status = Playing
                                    , track =
                                        Just
                                            { id = 7
                                            , title = Just "Orange Sun"
                                            , artist = Just "The Satsumas"
                                            , album = Just "Citrus"
                                            , durationMs = 185000
                                            }
                                    , positionMs = 42000
                                    , volume = 0.8
                                    , shuffle = True
                                    , repeat = RepeatQueue
                                    , stopAfterCurrent = False
                                    , queueLength = 13
                                    }
                                )
                            )
            , test "a stopped state has no track" <|
                \_ ->
                    state stoppedJson
                        |> Result.map (Maybe.map .track)
                        |> Expect.equal (Ok (Just Nothing))
            , test "every repeat mode decodes" <|
                \_ ->
                    [ "off", "queue", "track" ]
                        |> List.map (\mode -> state (stateJson "stopped" mode))
                        |> List.map (Result.map (Maybe.map .repeat))
                        |> Expect.equal
                            [ Ok (Just RepeatOff)
                            , Ok (Just RepeatQueue)
                            , Ok (Just RepeatTrack)
                            ]
            , test "an unknown status is an error, not a wrong state" <|
                \_ ->
                    state (stateJson "spinning" "off")
                        |> Result.map (Maybe.map .status)
                        |> Expect.equal (Ok Nothing)
            , test "another module's event is not ours" <|
                \_ ->
                    Player.handleEvent "library://scan-finished" Encode.null model
                        |> Expect.equal Nothing
            ]
        , describe "handleInvokeResult"
            [ test "a player error is shown" <|
                \_ ->
                    Player.handleInvokeResult "player_next" (Err "the player stopped") model
                        |> Maybe.map (Tuple.first >> .error)
                        |> Expect.equal (Just (Just "the player stopped"))
            , test "play_library belongs to the player" <|
                \_ ->
                    Player.handleInvokeResult "play_library" (Ok Encode.null) model
                        |> Maybe.map (Tuple.first >> .error)
                        |> Expect.equal (Just Nothing)
            , test "another module's command is not ours" <|
                \_ ->
                    Player.handleInvokeResult "list_folders" (Ok Encode.null) model
                        |> Expect.equal Nothing
            ]
        ]


model : Player.Model
model =
    Tuple.first Player.init


{-| Decodes a state event and returns the resulting state, or `Nothing` when
the payload was rejected.
-}
state : String -> Result Decode.Error (Maybe Player.State)
state json =
    Decode.decodeString Decode.value json
        |> Result.map
            (\payload ->
                Player.handleEvent "player://state" payload model
                    |> Maybe.map Tuple.first
                    |> Maybe.andThen
                        (\updated ->
                            if updated.error == Nothing then
                                Just updated.state

                            else
                                Nothing
                        )
            )


playingJson : String
playingJson =
    """
    { "status": "playing"
    , "track":
        { "id": 7
        , "title": "Orange Sun"
        , "artist": "The Satsumas"
        , "album": "Citrus"
        , "duration_ms": 185000
        }
    , "position_ms": 42000
    , "volume": 0.8
    , "shuffle": true
    , "repeat": "queue"
    , "stop_after_current": false
    , "queue_length": 13
    , "queue_index": 2
    , "error": null
    }
    """


stoppedJson : String
stoppedJson =
    stateJson "stopped" "off"


stateJson : String -> String -> String
stateJson status repeat =
    """
    { "status": \"""" ++ status ++ """"
    , "track": null
    , "position_ms": 0
    , "volume": 1.0
    , "shuffle": false
    , "repeat": \"""" ++ repeat ++ """"
    , "stop_after_current": false
    , "queue_length": 0
    , "queue_index": null
    , "error": null
    }
    """
