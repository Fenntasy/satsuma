port module Ports exposing (receive, send)

import Bridge exposing (Incoming, Outgoing)
import Json.Decode as Decode
import Json.Encode as Encode


port toJs : Encode.Value -> Cmd msg


port fromJs : (Decode.Value -> msg) -> Sub msg


send : Outgoing -> Cmd msg
send =
    Bridge.encodeOutgoing >> toJs


receive : (Result Decode.Error Incoming -> msg) -> Sub msg
receive toMsg =
    fromJs (Bridge.decodeIncoming >> toMsg)
