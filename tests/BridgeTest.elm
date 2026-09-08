module BridgeTest exposing (suite)

import Bridge exposing (Incoming(..), Outgoing(..))
import Expect
import Json.Decode as Decode
import Json.Encode as Encode
import Test exposing (Test, describe, test)


suite : Test
suite =
    describe "Bridge"
        [ describe "encodeOutgoing"
            [ test "Invoke carries the command and args" <|
                \_ ->
                    Invoke "ping" (Encode.object [ ( "n", Encode.int 1 ) ])
                        |> Bridge.encodeOutgoing
                        |> Encode.encode 0
                        |> Expect.equal "{\"tag\":\"invoke\",\"command\":\"ping\",\"args\":{\"n\":1}}"
            , test "SaveTheme carries the value" <|
                \_ ->
                    SaveTheme "dark"
                        |> Bridge.encodeOutgoing
                        |> Encode.encode 0
                        |> Expect.equal "{\"tag\":\"saveTheme\",\"value\":\"dark\"}"
            ]
        , describe "decodeIncoming"
            [ test "successful invoke result" <|
                \_ ->
                    decode "{\"tag\":\"invokeResult\",\"command\":\"ping\",\"ok\":true,\"payload\":\"pong\"}"
                        |> Result.map (mapPayload (Decode.decodeValue Decode.string))
                        |> Expect.equal (Ok (Just ( "ping", Ok (Ok "pong") )))
            , test "failed invoke result" <|
                \_ ->
                    decode "{\"tag\":\"invokeResult\",\"command\":\"ping\",\"ok\":false,\"error\":\"boom\"}"
                        |> Result.map (mapPayload (Decode.decodeValue Decode.string))
                        |> Expect.equal (Ok (Just ( "ping", Err "boom" )))
            , test "forwarded event" <|
                \_ ->
                    decode "{\"tag\":\"event\",\"name\":\"library://scan-finished\",\"payload\":{\"added\":1}}"
                        |> Result.map eventName
                        |> Expect.equal (Ok (Just "library://scan-finished"))
            , test "system theme change" <|
                \_ ->
                    decode "{\"tag\":\"systemTheme\",\"dark\":true}"
                        |> Expect.equal (Ok (SystemTheme True))
            , test "unknown tag is rejected" <|
                \_ ->
                    decode "{\"tag\":\"nope\"}"
                        |> Result.toMaybe
                        |> Expect.equal Nothing
            ]
        ]


decode : String -> Result Decode.Error Incoming
decode json =
    Decode.decodeString Decode.value json
        |> Result.andThen Bridge.decodeIncoming


mapPayload : (Decode.Value -> Result Decode.Error a) -> Incoming -> Maybe ( String, Result String (Result Decode.Error a) )
mapPayload f incoming =
    case incoming of
        InvokeResult command outcome ->
            Just ( command, Result.map f outcome )

        _ ->
            Nothing


eventName : Incoming -> Maybe String
eventName incoming =
    case incoming of
        Event name _ ->
            Just name

        _ ->
            Nothing
