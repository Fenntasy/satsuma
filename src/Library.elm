module Library exposing
    ( Folder
    , Model
    , Msg
    , Progress
    , Report
    , ScanState(..)
    , Stats
    , formatDuration
    , handleEvent
    , handleInvokeResult
    , init
    , reportText
    , update
    , view
    )

{-| The Library panel: library folders, scanning and a summary of what is in
the database.
-}

import Bridge exposing (Outgoing(..))
import Html exposing (Html, button, div, h2, li, p, progress, span, text, ul)
import Html.Attributes as Attr exposing (class, disabled, title, type_, value)
import Html.Events exposing (onClick)
import Json.Decode as Decode exposing (Decoder)
import Json.Encode as Encode
import Ports



-- MODEL


type alias Folder =
    { id : Int
    , path : String
    }


type alias Stats =
    { trackCount : Int
    , totalDurationMs : Int
    }


type alias Progress =
    { scanned : Int
    , total : Int
    , path : String
    }


type alias Report =
    { added : Int
    , updated : Int
    , removed : Int
    , failed : Int
    , unreachable : Int
    }


type ScanState
    = Idle
    | Scanning Progress
    | Finished Report
    | Failed String


type alias Model =
    { folders : List Folder
    , stats : Stats
    , scan : ScanState
    , error : Maybe String
    }


init : ( Model, Cmd Msg )
init =
    ( { folders = []
      , stats = { trackCount = 0, totalDurationMs = 0 }
      , scan = Idle
      , error = Nothing
      }
    , Cmd.batch [ refresh, startScan ]
    )


refresh : Cmd Msg
refresh =
    Cmd.batch
        [ Ports.send (Invoke "list_folders" (Encode.object []))
        , Ports.send (Invoke "library_stats" (Encode.object []))
        ]


startScan : Cmd Msg
startScan =
    Ports.send (Invoke "start_scan" (Encode.object []))



-- UPDATE


type Msg
    = PickFolder
    | RemoveFolder Int
    | StartScan


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        PickFolder ->
            ( model, Ports.send (Invoke "pick_folder" (Encode.object [])) )

        RemoveFolder id ->
            ( model
            , Ports.send (Invoke "remove_folder" (Encode.object [ ( "id", Encode.int id ) ]))
            )

        StartScan ->
            ( model, startScan )


{-| Handles the reply of a command this panel issued. Returns `Nothing` when
the command is not one of ours.
-}
handleInvokeResult : String -> Result String Decode.Value -> Model -> Maybe ( Model, Cmd Msg )
handleInvokeResult command outcome model =
    case command of
        "list_folders" ->
            Just (decodeInto (Decode.list folderDecoder) outcome model (\folders m -> ( { m | folders = folders }, Cmd.none )))

        "library_stats" ->
            Just (decodeInto statsDecoder outcome model (\stats m -> ( { m | stats = stats }, Cmd.none )))

        "pick_folder" ->
            Just
                (decodeInto (Decode.nullable Decode.string)
                    outcome
                    model
                    (\picked m ->
                        case picked of
                            Just path ->
                                ( m, Ports.send (Invoke "add_folder" (Encode.object [ ( "path", Encode.string path ) ])) )

                            Nothing ->
                                ( m, Cmd.none )
                    )
                )

        "add_folder" ->
            Just (decodeInto folderDecoder outcome model (\_ m -> ( m, Cmd.batch [ refresh, startScan ] )))

        "remove_folder" ->
            -- Rescan too: tracks shared with a folder that is still in the
            -- library go away with the removed one and must come back.
            Just (decodeInto (Decode.succeed ()) outcome model (\_ m -> ( m, Cmd.batch [ refresh, startScan ] )))

        "start_scan" ->
            Just (decodeInto (Decode.succeed ()) outcome model (\_ m -> ( m, Cmd.none )))

        _ ->
            Nothing


decodeInto : Decoder a -> Result String Decode.Value -> Model -> (a -> Model -> ( Model, Cmd Msg )) -> ( Model, Cmd Msg )
decodeInto decoder outcome model onValue =
    case outcome |> Result.andThen (Decode.decodeValue decoder >> Result.mapError Decode.errorToString) of
        Ok value ->
            onValue value { model | error = Nothing }

        Err error ->
            ( { model | error = Just error }, Cmd.none )


{-| Handles a Tauri event. Returns `Nothing` when the event is not one of ours.
-}
handleEvent : String -> Decode.Value -> Model -> Maybe ( Model, Cmd Msg )
handleEvent name payload model =
    case name of
        "library://scan-progress" ->
            Just
                ( case Decode.decodeValue progressDecoder payload of
                    Ok progressValue ->
                        { model | scan = Scanning progressValue }

                    Err error ->
                        { model | error = Just (Decode.errorToString error) }
                , Cmd.none
                )

        "library://scan-finished" ->
            Just
                ( case Decode.decodeValue outcomeDecoder payload of
                    Ok outcome ->
                        { model | scan = outcome }

                    Err error ->
                        { model | scan = Failed (Decode.errorToString error) }
                , refresh
                )

        _ ->
            Nothing



-- DECODERS


folderDecoder : Decoder Folder
folderDecoder =
    Decode.map2 Folder
        (Decode.field "id" Decode.int)
        (Decode.field "path" Decode.string)


statsDecoder : Decoder Stats
statsDecoder =
    Decode.map2 Stats
        (Decode.field "track_count" Decode.int)
        (Decode.field "total_duration_ms" Decode.int)


progressDecoder : Decoder Progress
progressDecoder =
    Decode.map3 Progress
        (Decode.field "scanned" Decode.int)
        (Decode.field "total" Decode.int)
        (Decode.field "path" Decode.string)


outcomeDecoder : Decoder ScanState
outcomeDecoder =
    Decode.field "status" Decode.string
        |> Decode.andThen
            (\status ->
                case status of
                    "finished" ->
                        Decode.map Finished
                            (Decode.map5 Report
                                (Decode.field "added" Decode.int)
                                (Decode.field "updated" Decode.int)
                                (Decode.field "removed" Decode.int)
                                (Decode.field "failed" Decode.int)
                                (Decode.field "unreachable" Decode.int)
                            )

                    "failed" ->
                        Decode.map Failed (Decode.field "message" Decode.string)

                    _ ->
                        Decode.fail ("Unknown scan status: " ++ status)
            )



-- VIEW


view : Model -> Html Msg
view model =
    div [ class "library" ]
        [ h2 [] [ text "Library" ]
        , p [ class "library-stats" ] [ text (statsText model.stats) ]
        , viewScan model.scan
        , viewError model.error
        , h2 [] [ text "Folders" ]
        , viewFolders model.folders
        , div [ class "library-actions" ]
            [ button [ type_ "button", class "button", onClick PickFolder ] [ text "Add folder" ]
            , button
                [ type_ "button"
                , class "button"
                , onClick StartScan
                , disabled (isScanning model.scan || List.isEmpty model.folders)
                ]
                [ text "Rescan" ]
            ]
        ]


statsText : Stats -> String
statsText stats =
    let
        tracks : String
        tracks =
            case stats.trackCount of
                1 ->
                    "1 track"

                n ->
                    String.fromInt n ++ " tracks"
    in
    tracks ++ " · " ++ formatDuration stats.totalDurationMs


{-| Formats a duration in milliseconds as `h:mm:ss` or `m:ss`.
-}
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


isScanning : ScanState -> Bool
isScanning scan =
    case scan of
        Scanning _ ->
            True

        _ ->
            False


viewScan : ScanState -> Html Msg
viewScan scan =
    case scan of
        Idle ->
            text ""

        Scanning { scanned, total, path } ->
            div [ class "scan" ]
                [ progress [ Attr.max (String.fromInt total), value (String.fromInt scanned) ] []
                , span [ class "scan-label", title path ]
                    [ text ("Scanning " ++ String.fromInt scanned ++ " / " ++ String.fromInt total) ]
                ]

        Finished report ->
            p [ class "scan-summary" ] [ text (reportText report) ]

        Failed error ->
            p [ class "error" ] [ text ("Scan failed: " ++ error) ]


{-| The one-line summary shown when a scan ends.
-}
reportText : Report -> String
reportText report =
    let
        counted : Int -> String -> List String -> List String
        counted count label rest =
            if count > 0 then
                (String.fromInt count ++ " " ++ label) :: rest

            else
                rest
    in
    "Scan finished: "
        ++ (counted report.added "added" []
                |> counted report.updated "updated"
                |> counted report.removed "removed"
                |> counted report.failed "unreadable"
                |> counted report.unreachable "folders unreachable"
                |> List.reverse
                |> String.join ", "
                |> (\summary ->
                        if String.isEmpty summary then
                            "nothing changed"

                        else
                            summary
                   )
           )


viewError : Maybe String -> Html Msg
viewError error =
    case error of
        Just message ->
            p [ class "error" ] [ text message ]

        Nothing ->
            text ""


viewFolders : List Folder -> Html Msg
viewFolders folders =
    if List.isEmpty folders then
        p [ class "muted" ] [ text "No folders yet. Add the folder containing your music." ]

    else
        ul [ class "folders" ] (List.map viewFolder folders)


{-| Wraps a path in left-to-right marks. The path is shown with
`direction: rtl` so it truncates on the left, which would otherwise move a
leading slash to the end of the line.
-}
isolated : String -> String
isolated path =
    "\u{200E}" ++ path ++ "\u{200E}"


viewFolder : Folder -> Html Msg
viewFolder folder =
    li [ class "folder" ]
        [ span [ class "folder-path", title folder.path ] [ text (isolated folder.path) ]
        , button
            [ type_ "button"
            , class "icon-button"
            , title "Remove folder"
            , onClick (RemoveFolder folder.id)
            ]
            [ text "✕" ]
        ]
