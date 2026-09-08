module Player exposing
    ( Model
    , Msg
    , Repeat(..)
    , State
    , Status(..)
    , Track
    , formatPosition
    , handleEvent
    , handleInvokeResult
    , init
    , playLibrary
    , update
    , view
    )

{-| The player bar: what is playing, transport controls, seek and volume.

The backend owns playback; this module mirrors the state it sends and turns
clicks into commands.

-}

import Bridge exposing (Outgoing(..))
import Html exposing (Html, button, div, input, span, text)
import Html.Attributes as Attr exposing (class, classList, title, type_, value)
import Html.Events exposing (onClick, onInput)
import Json.Decode as Decode exposing (Decoder)
import Json.Encode as Encode
import Ports



-- MODEL


type alias Track =
    { id : Int
    , title : Maybe String
    , artist : Maybe String
    , album : Maybe String
    , durationMs : Int
    }


type Status
    = Stopped
    | Playing
    | Paused


type Repeat
    = RepeatOff
    | RepeatTrack
    | RepeatQueue


type alias State =
    { status : Status
    , track : Maybe Track
    , positionMs : Int
    , volume : Float
    , shuffle : Bool
    , repeat : Repeat
    , stopAfterCurrent : Bool
    , queueLength : Int
    , error : Maybe String
    }


type alias Model =
    { state : State
    , error : Maybe String

    -- While the user drags the seek bar, the backend position is ignored so
    -- the handle does not jump back under the pointer.
    , seeking : Maybe Int
    }


emptyState : State
emptyState =
    { status = Stopped
    , track = Nothing
    , positionMs = 0
    , volume = 1.0
    , shuffle = False
    , repeat = RepeatOff
    , stopAfterCurrent = False
    , queueLength = 0
    , error = Nothing
    }


init : ( Model, Cmd Msg )
init =
    ( { state = emptyState, error = Nothing, seeking = Nothing }
    , Ports.send (Invoke "player_state" (Encode.object []))
    )


{-| Queues the whole library and starts playing it.
-}
playLibrary : Cmd msg
playLibrary =
    Ports.send (Invoke "play_library" (Encode.object []))



-- UPDATE


type Msg
    = PlayPause
    | Next
    | Previous
    | Stop
    | SeekTo Int
    | SeekPreview Int
    | SetVolume Float
    | ToggleShuffle
    | CycleRepeat
    | ToggleStopAfterCurrent


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        PlayPause ->
            ( model, command "player_play_pause" [] )

        Next ->
            ( model, command "player_next" [] )

        Previous ->
            ( model, command "player_previous" [] )

        Stop ->
            ( model, command "player_stop" [] )

        SeekPreview positionMs ->
            ( { model | seeking = Just positionMs }, Cmd.none )

        SeekTo positionMs ->
            ( { model | seeking = Nothing }
            , command "player_seek" [ ( "positionMs", Encode.int positionMs ) ]
            )

        SetVolume volume ->
            ( { model | state = setVolume volume model.state }
            , command "player_set_volume" [ ( "volume", Encode.float volume ) ]
            )

        ToggleShuffle ->
            ( model
            , command "player_set_shuffle"
                [ ( "shuffle", Encode.bool (not model.state.shuffle) ) ]
            )

        CycleRepeat ->
            ( model
            , command "player_set_repeat"
                [ ( "repeat", Encode.string (repeatToString (cycleRepeat model.state.repeat)) ) ]
            )

        ToggleStopAfterCurrent ->
            ( model
            , command "player_set_stop_after_current"
                [ ( "stop", Encode.bool (not model.state.stopAfterCurrent) ) ]
            )


setVolume : Float -> State -> State
setVolume volume state =
    { state | volume = volume }


command : String -> List ( String, Encode.Value ) -> Cmd msg
command name args =
    Ports.send (Invoke name (Encode.object args))


cycleRepeat : Repeat -> Repeat
cycleRepeat repeat =
    case repeat of
        RepeatOff ->
            RepeatQueue

        RepeatQueue ->
            RepeatTrack

        RepeatTrack ->
            RepeatOff


repeatToString : Repeat -> String
repeatToString repeat =
    case repeat of
        RepeatOff ->
            "off"

        RepeatTrack ->
            "track"

        RepeatQueue ->
            "queue"


{-| Handles the reply of a command this module issued. Returns `Nothing`
when the command is not one of ours.
-}
handleInvokeResult : String -> Result String Decode.Value -> Model -> Maybe ( Model, Cmd Msg )
handleInvokeResult command_ outcome model =
    if String.startsWith "player_" command_ || command_ == "play_library" then
        Just
            (case outcome of
                Ok _ ->
                    ( { model | error = Nothing }, Cmd.none )

                Err error ->
                    ( { model | error = Just error }, Cmd.none )
            )

    else
        Nothing


{-| Handles a player event. Returns `Nothing` when the event is not ours.
-}
handleEvent : String -> Decode.Value -> Model -> Maybe ( Model, Cmd Msg )
handleEvent name payload model =
    if name == "player://state" then
        Just
            ( case Decode.decodeValue stateDecoder payload of
                Ok state ->
                    -- The backend reports playback failures in the state,
                    -- e.g. a file that moved since it was scanned.
                    { model | state = state, error = state.error }

                Err error ->
                    { model | error = Just (Decode.errorToString error) }
            , Cmd.none
            )

    else
        Nothing



-- DECODERS


stateDecoder : Decoder State
stateDecoder =
    Decode.map2
        (\state error -> { state | error = error })
        (Decode.map8 State
            (Decode.field "status" statusDecoder)
            (Decode.field "track" (Decode.nullable trackDecoder))
            (Decode.field "position_ms" Decode.int)
            (Decode.field "volume" Decode.float)
            (Decode.field "shuffle" Decode.bool)
            (Decode.field "repeat" repeatDecoder)
            (Decode.field "stop_after_current" Decode.bool)
            (Decode.field "queue_length" Decode.int)
            |> Decode.map (\build -> build Nothing)
        )
        (Decode.field "error" (Decode.nullable Decode.string))


statusDecoder : Decoder Status
statusDecoder =
    Decode.string
        |> Decode.andThen
            (\raw ->
                case raw of
                    "stopped" ->
                        Decode.succeed Stopped

                    "playing" ->
                        Decode.succeed Playing

                    "paused" ->
                        Decode.succeed Paused

                    _ ->
                        Decode.fail ("Unknown player status: " ++ raw)
            )


repeatDecoder : Decoder Repeat
repeatDecoder =
    Decode.string
        |> Decode.andThen
            (\raw ->
                case raw of
                    "off" ->
                        Decode.succeed RepeatOff

                    "track" ->
                        Decode.succeed RepeatTrack

                    "queue" ->
                        Decode.succeed RepeatQueue

                    _ ->
                        Decode.fail ("Unknown repeat mode: " ++ raw)
            )


trackDecoder : Decoder Track
trackDecoder =
    Decode.map5 Track
        (Decode.field "id" Decode.int)
        (Decode.field "title" (Decode.nullable Decode.string))
        (Decode.field "artist" (Decode.nullable Decode.string))
        (Decode.field "album" (Decode.nullable Decode.string))
        (Decode.field "duration_ms" Decode.int)



-- VIEW


view : Model -> Html Msg
view model =
    let
        state : State
        state =
            model.state

        position : Int
        position =
            Maybe.withDefault state.positionMs model.seeking

        duration : Int
        duration =
            state.track |> Maybe.map .durationMs |> Maybe.withDefault 0
    in
    div [ class "player" ]
        [ div [ class "player-track" ] [ viewTrack state ]
        , div [ class "player-transport" ]
            [ toggle "Shuffle" "⤨" state.shuffle ToggleShuffle
            , iconButton "Previous" "⏮" Previous
            , playPauseButton state.status
            , iconButton "Next" "⏭" Next
            , iconButton "Stop" "⏹" Stop
            , toggle "Stop after this track" "⏏" state.stopAfterCurrent ToggleStopAfterCurrent
            , repeatButton state.repeat
            ]
        , div [ class "player-seek" ]
            [ span [ class "player-time" ] [ text (formatPosition position) ]
            , input
                [ type_ "range"
                , class "seek"
                , Attr.min "0"
                , Attr.max (String.fromInt (max duration 1))
                , value (String.fromInt position)
                , Attr.disabled (duration == 0)
                , onInput (String.toInt >> Maybe.withDefault 0 >> SeekPreview)

                -- `change` fires for the keyboard too, unlike `mouseup`,
                -- and always ends the drag.
                , Html.Events.on "change" (Decode.succeed (SeekTo position))
                ]
                []
            , span [ class "player-time" ] [ text (formatPosition duration) ]
            ]
        , div [ class "player-volume" ]
            [ span [ title "Volume" ] [ text "🔊" ]
            , input
                [ type_ "range"
                , class "volume"
                , Attr.min "0"
                , Attr.max "100"
                , value (String.fromInt (round (state.volume * 100)))
                , onInput
                    (String.toFloat
                        >> Maybe.withDefault 100
                        >> (\percent -> SetVolume (percent / 100))
                    )
                ]
                []
            ]
        , viewError model.error
        ]


viewTrack : State -> Html Msg
viewTrack state =
    case state.track of
        Nothing ->
            span [ class "muted" ]
                [ text
                    (if state.queueLength == 0 then
                        "Nothing playing"

                     else
                        "Queue ready"
                    )
                ]

        Just track ->
            div [ class "now-playing" ]
                [ span [ class "now-playing-title" ]
                    [ text (Maybe.withDefault "Unknown title" track.title) ]
                , span [ class "now-playing-artist" ]
                    [ text (Maybe.withDefault "Unknown artist" track.artist) ]
                ]


playPauseButton : Status -> Html Msg
playPauseButton status =
    case status of
        Playing ->
            iconButton "Pause" "⏸" PlayPause

        _ ->
            iconButton "Play" "▶" PlayPause


iconButton : String -> String -> Msg -> Html Msg
iconButton label icon msg =
    button
        [ type_ "button", class "player-button", title label, onClick msg ]
        [ text icon ]


toggle : String -> String -> Bool -> Msg -> Html Msg
toggle label icon isOn msg =
    button
        [ type_ "button"
        , classList [ ( "player-button", True ), ( "is-active", isOn ) ]
        , title label
        , onClick msg
        ]
        [ text icon ]


repeatButton : Repeat -> Html Msg
repeatButton repeat =
    let
        ( label, icon ) =
            case repeat of
                RepeatOff ->
                    ( "Repeat off", "🔁" )

                RepeatQueue ->
                    ( "Repeat playlist", "🔁" )

                RepeatTrack ->
                    ( "Repeat track", "🔂" )
    in
    button
        [ type_ "button"
        , classList [ ( "player-button", True ), ( "is-active", repeat /= RepeatOff ) ]
        , title label
        , onClick CycleRepeat
        ]
        [ text icon ]


viewError : Maybe String -> Html Msg
viewError error =
    case error of
        Just message ->
            span [ class "error" ] [ text message ]

        Nothing ->
            text ""


{-| Formats a position in milliseconds as `m:ss`, or `h:mm:ss` past an hour.
-}
formatPosition : Int -> String
formatPosition ms =
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
