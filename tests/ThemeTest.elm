module ThemeTest exposing (suite)

import Expect
import Json.Decode as Decode
import Test exposing (Test, describe, test)
import Theme exposing (Mode(..), Setting(..))


suite : Test
suite =
    describe "Theme"
        [ describe "resolve"
            [ test "System follows a dark OS preference" <|
                \_ -> Theme.resolve True System |> Expect.equal DarkMode
            , test "System follows a light OS preference" <|
                \_ -> Theme.resolve False System |> Expect.equal LightMode
            , test "Light ignores the OS preference" <|
                \_ -> Theme.resolve True Light |> Expect.equal LightMode
            , test "Dark ignores the OS preference" <|
                \_ -> Theme.resolve False Dark |> Expect.equal DarkMode
            ]
        , describe "cycle"
            [ test "visits every setting and returns to the start" <|
                \_ ->
                    System
                        |> Theme.cycle
                        |> Theme.cycle
                        |> Theme.cycle
                        |> Expect.equal System
            , test "goes System -> Light -> Dark" <|
                \_ ->
                    [ System, Theme.cycle System, Theme.cycle (Theme.cycle System) ]
                        |> Expect.equal [ System, Light, Dark ]
            ]
        , describe "string round-trip"
            (List.map
                (\setting ->
                    test (Theme.toString setting) <|
                        \_ ->
                            Theme.fromString (Theme.toString setting)
                                |> Expect.equal (Just setting)
                )
                [ System, Light, Dark ]
            )
        , describe "decoder"
            [ test "accepts a known value" <|
                \_ ->
                    Decode.decodeString Theme.decoder "\"dark\""
                        |> Expect.equal (Ok Dark)
            , test "rejects an unknown value" <|
                \_ ->
                    Decode.decodeString Theme.decoder "\"sepia\""
                        |> Result.toMaybe
                        |> Expect.equal Nothing
            ]
        ]
