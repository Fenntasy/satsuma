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
    , refresh
    , reportText
    , update
    , view
    )

{-| The Library panel: library folders, scanning and a summary of what is in
the database.
-}

import Bridge exposing (Outgoing(..))
import Html exposing (Html, button, div, h2, input, li, p, progress, span, text, ul)
import Html.Attributes as Attr exposing (class, disabled, title, type_, value)
import Html.Events exposing (onClick, onDoubleClick, onInput)
import Html.Lazy
import Json.Decode as Decode exposing (Decoder)
import Json.Encode as Encode
import Player
import Ports
import Set exposing (Set)
import Tree exposing (Children(..), Grouping(..))



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
    , emptied : Int
    }


type ScanState
    = -- Asked for, but the backend has not reported progress yet: it is
      -- still walking the folders.
      Requested
    | Scanning Progress
    | Finished Report
    | Failed String


type alias Model =
    { folders : List Folder
    , stats : Stats
    , scan : ScanState
    , error : Maybe String
    , rows : List Tree.Row

    -- Sorted for the current grouping, so typing in the filter does not
    -- sort the whole library again on every keystroke.
    , sortedRows : List Tree.Row
    , grouping : Grouping
    , filter : String

    -- Paths of the branches the user opened, so a rescan does not close
    -- what they were looking at.
    , expanded : Set String
    }


init : ( Model, Cmd Msg )
init =
    ( { folders = []
      , stats = { trackCount = 0, totalDurationMs = 0 }
      , scan = Requested
      , error = Nothing
      , rows = []
      , sortedRows = []
      , grouping = GenreArtistAlbum
      , filter = ""
      , expanded = Set.empty
      }
    , Cmd.batch [ refresh, startScan ]
    )


refresh : Cmd Msg
refresh =
    Cmd.batch
        [ Ports.send (Invoke "list_folders" (Encode.object []))
        , Ports.send (Invoke "library_stats" (Encode.object []))
        , Ports.send (Invoke "library_rows" (Encode.object []))
        ]


startScan : Cmd Msg
startScan =
    Ports.send (Invoke "start_scan" (Encode.object []))



-- UPDATE


type Msg
    = PickFolder
    | RemoveFolder Int
    | StartScan
    | PlayLibrary
    | EnqueueLibrary
    | Toggle String
    | Play (List Int) (Maybe Int)
    | Enqueue (List Int)
    | AddToPlaylist Int (List Int)
    | SetFilter String
    | SetGrouping Grouping


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
            ( { model | scan = Requested }, startScan )

        PlayLibrary ->
            ( model, Player.playLibrary )

        EnqueueLibrary ->
            ( model, Player.enqueueLibrary )

        Toggle path ->
            ( { model | expanded = toggle path model.expanded }, Cmd.none )

        Play ids startId ->
            ( model
            , Ports.send
                (Invoke "play_tracks"
                    (Encode.object
                        [ ( "ids", Encode.list Encode.int ids )
                        , ( "startId"
                          , Maybe.map Encode.int startId |> Maybe.withDefault Encode.null
                          )
                        ]
                    )
                )
            )

        Enqueue ids ->
            ( model, tracksCommand "enqueue_tracks" ids )

        AddToPlaylist playlistId ids ->
            ( model
            , Ports.send
                (Invoke "add_to_playlist"
                    (Encode.object
                        [ ( "id", Encode.int playlistId )
                        , ( "ids", Encode.list Encode.int ids )
                        ]
                    )
                )
            )

        SetFilter filter ->
            ( { model | filter = filter }, Cmd.none )

        SetGrouping grouping ->
            ( { model
                | grouping = grouping
                , sortedRows = Tree.sortFor grouping model.rows
              }
            , Cmd.none
            )


toggle : String -> Set String -> Set String
toggle path expanded =
    if Set.member path expanded then
        Set.remove path expanded

    else
        Set.insert path expanded


tracksCommand : String -> List Int -> Cmd Msg
tracksCommand name ids =
    Ports.send (Invoke name (Encode.object [ ( "ids", Encode.list Encode.int ids ) ]))


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

        "library_rows" ->
            Just
                (decodeInto (Decode.list Tree.rowDecoder)
                    outcome
                    model
                    (\rows m ->
                        ( { m | rows = rows, sortedRows = Tree.sortFor m.grouping rows }
                        , Cmd.none
                        )
                    )
                )

        "play_tracks" ->
            Just (decodeInto (Decode.succeed ()) outcome model (\_ m -> ( m, Cmd.none )))

        "enqueue_tracks" ->
            Just (decodeInto (Decode.succeed ()) outcome model (\_ m -> ( m, Cmd.none )))

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
            Just (decodeInto folderDecoder outcome model (\_ m -> ( { m | scan = Requested }, Cmd.batch [ refresh, startScan ] )))

        "remove_folder" ->
            -- Rescan too: tracks shared with a folder that is still in the
            -- library go away with the removed one and must come back.
            Just (decodeInto (Decode.succeed ()) outcome model (\_ m -> ( { m | scan = Requested }, Cmd.batch [ refresh, startScan ] )))

        "start_scan" ->
            -- A rejected request must leave the panel usable: staying in
            -- `Requested` would disable Rescan for good.
            Just
                (case outcome of
                    Ok _ ->
                        ( { model | error = Nothing }, Cmd.none )

                    Err error ->
                        ( { model | scan = Failed error, error = Just error }, Cmd.none )
                )

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
                        { model
                            | scan = Failed (Decode.errorToString error)
                            , error = Just (Decode.errorToString error)
                        }
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
                            (Decode.map6 Report
                                (Decode.field "added" Decode.int)
                                (Decode.field "updated" Decode.int)
                                (Decode.field "removed" Decode.int)
                                (Decode.field "failed" Decode.int)
                                (Decode.field "unreachable" Decode.int)
                                (Decode.field "emptied" Decode.int)
                            )

                    "failed" ->
                        Decode.map Failed (Decode.field "message" Decode.string)

                    _ ->
                        Decode.fail ("Unknown scan status: " ++ status)
            )



-- VIEW


{-| `activePlaylist` is the id of the open playlist, or zero when none is
open: a plain Int so the lazy tree below can compare it.
-}
view : Int -> Model -> Html Msg
view activePlaylist model =
    div [ class "library" ]
        [ h2 [] [ text "Library" ]
        , p [ class "library-stats" ] [ text (statsText model.stats) ]
        , viewScan model.scan
        , viewError model.error
        , h2 [] [ text "Folders" ]
        , viewFolders model.folders
        , div [ class "library-actions" ]
            [ button
                [ type_ "button"
                , class "button is-primary"
                , onClick PlayLibrary
                , disabled (model.stats.trackCount == 0)
                ]
                [ text "Play all" ]
            , button
                [ type_ "button"
                , class "button"
                , onClick EnqueueLibrary
                , disabled (model.stats.trackCount == 0)
                , title "Add every track to the end of the queue"
                ]
                [ text "Queue all" ]
            , button [ type_ "button", class "button", onClick PickFolder ] [ text "Add folder" ]
            , button
                [ type_ "button"
                , class "button"
                , onClick StartScan
                , disabled (isScanning model.scan || List.isEmpty model.folders)
                ]
                [ text "Rescan" ]
            ]
        , viewTree activePlaylist model
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
        Requested ->
            True

        Scanning _ ->
            True

        _ ->
            False


viewScan : ScanState -> Html Msg
viewScan scan =
    case scan of
        Requested ->
            p [ class "scan-summary" ] [ text "Looking for music…" ]

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
                |> counted report.unreachable
                    (if report.unreachable == 1 then
                        "folder unreachable"

                     else
                        "folders unreachable"
                    )
                |> counted report.emptied
                    (if report.emptied == 1 then
                        "folder looks empty, tracks kept"

                     else
                        "folders look empty, tracks kept"
                    )
                |> List.reverse
                |> String.join ", "
                |> (\summary ->
                        if String.isEmpty summary then
                            "nothing changed"

                        else
                            summary
                   )
           )


viewTree : Int -> Model -> Html Msg
viewTree activePlaylist model =
    -- Rebuilding the tree is the expensive part of this panel, and the
    -- player reports its position several times a second: only redo it
    -- when what it is built from changed.
    Html.Lazy.lazy5 viewTreeFor activePlaylist model.sortedRows model.grouping model.filter model.expanded


viewTreeFor : Int -> List Tree.Row -> Grouping -> String -> Set String -> Html Msg
viewTreeFor activePlaylist rows grouping filter expanded =
    let
        nodes : List Tree.Node
        nodes =
            Tree.buildSorted grouping filter rows
    in
    div [ class "tree-panel" ]
        [ h2 [] [ text "Browse" ]
        , div [ class "tree-controls" ]
            [ input
                [ type_ "search"
                , class "tree-filter"
                , Attr.placeholder "Filter"
                , Attr.value filter
                , Attr.attribute "aria-label" "Filter the library"
                , onInput SetFilter
                ]
                []
            , Html.select
                [ class "tree-grouping"
                , Attr.attribute "aria-label" "Group the library by"
                , onInput (groupingFromLabel >> SetGrouping)
                ]
                (List.map (viewGroupingOption grouping) Tree.groupings)
            ]
        , if List.isEmpty nodes then
            p [ class "muted" ] [ text (emptyTreeText rows) ]

          else
            ul [ class "tree" ] (List.map (viewNode activePlaylist expanded []) nodes)
        ]


emptyTreeText : List Tree.Row -> String
emptyTreeText rows =
    if List.isEmpty rows then
        "Nothing scanned yet."

    else
        "Nothing matches that filter."


viewGroupingOption : Grouping -> Grouping -> Html Msg
viewGroupingOption current grouping =
    Html.option
        [ Attr.value (Tree.groupingLabel grouping)
        , Attr.selected (current == grouping)
        ]
        [ text (Tree.groupingLabel grouping) ]


groupingFromLabel : String -> Grouping
groupingFromLabel label =
    Tree.groupings
        |> List.filter (\grouping -> Tree.groupingLabel grouping == label)
        |> List.head
        |> Maybe.withDefault GenreArtistAlbum


{-| `siblings` is what the enclosing branch holds, so choosing a track plays
the rest of its album after it rather than that track alone.
-}
viewNode : Int -> Set String -> List Int -> Tree.Node -> Html Msg
viewNode activePlaylist expanded siblings node =
    let
        isOpen : Bool
        isOpen =
            Set.member node.path expanded

        ids : List Int
        ids =
            node.ids
    in
    li [ class "tree-node" ]
        [ div [ class "tree-row" ]
            [ case node.children of
                Track ->
                    span [ class "tree-bullet" ] [ text "♪" ]

                Branches _ ->
                    button
                        [ type_ "button"
                        , class "tree-twisty"
                        , Attr.attribute "aria-expanded"
                            (if isOpen then
                                "true"

                             else
                                "false"
                            )
                        , title
                            (if isOpen then
                                "Collapse"

                             else
                                "Expand"
                            )
                        , onClick (Toggle node.path)
                        ]
                        [ text
                            (if isOpen then
                                "▾"

                             else
                                "▸"
                            )
                        ]
            , button
                ([ type_ "button"
                 , class "tree-label"

                 -- The full name: the sidebar is narrow and long album
                 -- titles all truncate to the same thing.
                 , title (node.label ++ hint node.children)
                 ]
                    ++ (case node.children of
                            Track ->
                                -- Playing on a single click would restart
                                -- the track on the second half of a double
                                -- click, which is an audible stutter.
                                [ onDoubleClick (Play (queueFor siblings ids) (List.head ids)) ]

                            Branches _ ->
                                [ onClick (Toggle node.path)
                                , onDoubleClick (Play ids Nothing)
                                ]
                       )
                )
                [ text node.label ]
            , viewCount node
            , button
                [ type_ "button"
                , class "icon-button tree-add"
                , title (addLabel activePlaylist node.label)
                , onClick
                    (if activePlaylist > 0 then
                        AddToPlaylist activePlaylist ids

                     else
                        Enqueue ids
                    )
                ]
                [ text "+" ]
            ]
        , case node.children of
            Branches children ->
                if isOpen then
                    ul [ class "tree" ] (List.map (viewNode activePlaylist expanded node.ids) children)

                else
                    text ""

            Track ->
                text ""
        ]


{-| Where the button adds to: the open playlist when there is one, and the
queue otherwise.
-}
addLabel : Int -> String -> String
addLabel activePlaylist label =
    if activePlaylist > 0 then
        "Add " ++ label ++ " to the playlist"

    else
        "Add " ++ label ++ " to the queue"


{-| Says how to play a row, since a single click only opens a branch.
-}
hint : Children -> String
hint children =
    case children of
        Track ->
            " (double-click to play)"

        Branches _ ->
            ""


{-| What to queue when a track is chosen: its siblings when it has any, so
the album carries on, and otherwise the track itself.
-}
queueFor : List Int -> List Int -> List Int
queueFor siblings ids =
    if List.isEmpty siblings then
        ids

    else
        siblings


viewCount : Tree.Node -> Html Msg
viewCount node =
    case node.children of
        Track ->
            span [ class "tree-count" ] [ text (formatDuration node.durationMs) ]

        Branches _ ->
            span [ class "tree-count" ] [ text (String.fromInt node.trackCount) ]


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
