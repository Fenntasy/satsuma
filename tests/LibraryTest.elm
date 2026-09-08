module LibraryTest exposing (suite)

import Expect
import Json.Decode as Decode
import Json.Encode as Encode
import Library exposing (ScanState(..))
import Test exposing (Test, describe, test)


suite : Test
suite =
    describe "Library"
        [ describe "formatDuration"
            [ test "minutes and seconds" <|
                \_ -> Library.formatDuration 754000 |> Expect.equal "12:34"
            , test "pads seconds" <|
                \_ -> Library.formatDuration 65000 |> Expect.equal "1:05"
            , test "hours" <|
                \_ -> Library.formatDuration 3723000 |> Expect.equal "1:02:03"
            , test "zero" <|
                \_ -> Library.formatDuration 0 |> Expect.equal "0:00"
            ]
        , describe "handleInvokeResult"
            [ test "list_folders fills the folders" <|
                \_ ->
                    json "[{\"id\":1,\"path\":\"/music\"}]"
                        |> Result.map (\value -> Library.handleInvokeResult "list_folders" (Ok value) model)
                        |> Expect.equal (Ok (Just ( { model | folders = [ { id = 1, path = "/music" } ] }, Cmd.none )))
            , test "library_stats fills the stats" <|
                \_ ->
                    json "{\"track_count\":13000,\"total_duration_ms\":42}"
                        |> Result.map (\value -> Library.handleInvokeResult "library_stats" (Ok value) model)
                        |> Result.map (Maybe.map (Tuple.first >> .stats))
                        |> Expect.equal (Ok (Just { trackCount = 13000, totalDurationMs = 42 }))
            , test "a backend error is shown" <|
                \_ ->
                    Library.handleInvokeResult "add_folder" (Err "folder already in the library") model
                        |> Maybe.map (Tuple.first >> .error)
                        |> Expect.equal (Just (Just "folder already in the library"))
            , test "a successful reply clears a previous error" <|
                \_ ->
                    json "[]"
                        |> Result.map (\value -> Library.handleInvokeResult "list_folders" (Ok value) { model | error = Just "old" })
                        |> Result.map (Maybe.map (Tuple.first >> .error))
                        |> Expect.equal (Ok (Just Nothing))
            , test "unknown commands are not ours" <|
                \_ ->
                    Library.handleInvokeResult "ping" (Ok Encode.null) model
                        |> Expect.equal Nothing
            ]
        , describe "handleEvent"
            [ test "progress updates the scan state" <|
                \_ ->
                    json "{\"scanned\":3,\"total\":10,\"path\":\"/music/a.mp3\"}"
                        |> Result.map (\value -> Library.handleEvent "library://scan-progress" value model)
                        |> Result.map (Maybe.map (Tuple.first >> .scan))
                        |> Expect.equal (Ok (Just (Scanning { scanned = 3, total = 10, path = "/music/a.mp3" })))
            , test "finished report" <|
                \_ ->
                    json "{\"status\":\"finished\",\"added\":1,\"updated\":2,\"removed\":3,\"failed\":0}"
                        |> Result.map (\value -> Library.handleEvent "library://scan-finished" value model)
                        |> Result.map (Maybe.map (Tuple.first >> .scan))
                        |> Expect.equal (Ok (Just (Finished { added = 1, updated = 2, removed = 3, failed = 0 })))
            , test "failed scan" <|
                \_ ->
                    json "{\"status\":\"failed\",\"message\":\"disk on fire\"}"
                        |> Result.map (\value -> Library.handleEvent "library://scan-finished" value model)
                        |> Result.map (Maybe.map (Tuple.first >> .scan))
                        |> Expect.equal (Ok (Just (Failed "disk on fire")))
            , test "unknown events are not ours" <|
                \_ ->
                    Library.handleEvent "player://tick" Encode.null model
                        |> Expect.equal Nothing
            ]
        ]


model : Library.Model
model =
    Tuple.first Library.init


json : String -> Result Decode.Error Decode.Value
json =
    Decode.decodeString Decode.value
