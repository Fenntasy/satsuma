module Bridge exposing (Incoming(..), Outgoing(..), decodeIncoming, encodeOutgoing)

{-| The message protocol between Elm and the thin JS layer (`bridge.js`).

Every message is a JSON object with a `tag` field. Outgoing `Invoke` messages
call a Tauri command; the JS side answers with an `InvokeResult` carrying the
same command name.

-}

import Json.Decode as Decode exposing (Decoder)
import Json.Encode as Encode


type Outgoing
    = Invoke String Encode.Value
    | SaveTheme String


type Incoming
    = InvokeResult String (Result String Decode.Value)
    | SystemTheme Bool


encodeOutgoing : Outgoing -> Encode.Value
encodeOutgoing outgoing =
    case outgoing of
        Invoke command args ->
            Encode.object
                [ ( "tag", Encode.string "invoke" )
                , ( "command", Encode.string command )
                , ( "args", args )
                ]

        SaveTheme setting ->
            Encode.object
                [ ( "tag", Encode.string "saveTheme" )
                , ( "value", Encode.string setting )
                ]


decodeIncoming : Decode.Value -> Result Decode.Error Incoming
decodeIncoming =
    Decode.decodeValue incomingDecoder


incomingDecoder : Decoder Incoming
incomingDecoder =
    Decode.field "tag" Decode.string
        |> Decode.andThen
            (\tag ->
                case tag of
                    "invokeResult" ->
                        Decode.map2 InvokeResult
                            (Decode.field "command" Decode.string)
                            invokeOutcomeDecoder

                    "systemTheme" ->
                        Decode.map SystemTheme (Decode.field "dark" Decode.bool)

                    _ ->
                        Decode.fail ("Unknown incoming tag: " ++ tag)
            )


invokeOutcomeDecoder : Decoder (Result String Decode.Value)
invokeOutcomeDecoder =
    Decode.field "ok" Decode.bool
        |> Decode.andThen
            (\ok ->
                if ok then
                    Decode.map Ok (Decode.field "payload" Decode.value)

                else
                    Decode.map Err (Decode.field "error" Decode.string)
            )
