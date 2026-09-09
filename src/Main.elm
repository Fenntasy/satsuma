module Main exposing (BackendStatus, Flags, Model, Msg, Panel, main)

import Bridge exposing (Incoming(..), Outgoing(..))
import Browser
import Browser.Dom
import Dict exposing (Dict)
import Html exposing (Html, button, div, h1, h2, input, main_, nav, p, span, text)
import Html.Attributes as Attr exposing (attribute, class, classList, title, type_)
import Html.Events exposing (onClick, onDoubleClick, onInput)
import Json.Decode as Decode
import Json.Encode as Encode
import Library
import Player
import Playlist exposing (Column)
import Ports
import Task
import Theme exposing (Mode(..), Setting(..))
import Tree


main : Program Flags Model Msg
main =
    Browser.element
        { init = init
        , update = update
        , view = view
        , subscriptions = subscriptions
        }



-- MODEL


type alias Flags =
    { theme : Maybe String
    , systemDark : Bool

    -- A JSON object of column name to width: flags cannot carry a Dict.
    , widths : Decode.Value
    }


type Panel
    = Library
    | NowPlaying
    | Playlists


type alias Model =
    { theme : Setting
    , systemDark : Bool
    , panel : Panel
    , backend : BackendStatus
    , library : Library.Model
    , player : Player.Model
    , playlists : List Playlist.Playlist
    , activePlaylist : Maybe Int
    , sort : Maybe Playlist.Sort
    , widths : Dict String Int
    , renaming : Maybe ( Int, String )
    , playlistError : Maybe String
    }


type BackendStatus
    = Connecting
    | Connected String
    | Unreachable String


init : Flags -> ( Model, Cmd Msg )
init flags =
    let
        ( library, libraryCmd ) =
            Library.init

        ( player, playerCmd ) =
            Player.init
    in
    ( { theme =
            flags.theme
                |> Maybe.andThen Theme.fromString
                |> Maybe.withDefault System
      , systemDark = flags.systemDark
      , panel = Library
      , backend = Connecting
      , library = library
      , player = player
      , playlists = []
      , activePlaylist = Nothing
      , sort = Nothing
      , widths =
            Decode.decodeValue (Decode.dict Decode.int) flags.widths
                |> Result.withDefault Dict.empty
      , renaming = Nothing
      , playlistError = Nothing
      }
    , Cmd.batch
        [ Ports.send (Invoke "ping" (Encode.object []))
        , Cmd.map LibraryMsg libraryCmd
        , Cmd.map PlayerMsg playerCmd
        , listPlaylists
        ]
    )



-- UPDATE


type Msg
    = SelectPanel Panel
    | CycleTheme
    | LibraryMsg Library.Msg
    | PlayerMsg Player.Msg
    | SelectPlaylist Int
    | NewPlaylist
    | DeletePlaylist Int
    | StartRenaming Int String
    | EditRename String
    | CommitRename
    | CancelRename
    | SortBy Column
    | Rate Int (Maybe Int)
    | PlayFrom Int
    | SetWidth Column Int
    | Focused
    | FromJs (Result Decode.Error Incoming)


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        SelectPanel panel ->
            ( { model | panel = panel }, Cmd.none )

        CycleTheme ->
            let
                next : Setting
                next =
                    Theme.cycle model.theme
            in
            ( { model | theme = next }, Ports.send (SaveTheme (Theme.toString next)) )

        LibraryMsg libraryMsg ->
            Library.update libraryMsg model.library
                |> updateLibrary model

        PlayerMsg playerMsg ->
            Player.update playerMsg model.player
                |> updatePlayer model

        SelectPlaylist id ->
            ( { model | activePlaylist = Just id, sort = Nothing }, Cmd.none )

        NewPlaylist ->
            ( model
            , invoke "create_playlist"
                [ ( "name", Encode.string (nextPlaylistName model.playlists) ) ]
            )

        DeletePlaylist id ->
            ( { model | activePlaylist = Nothing }
            , invoke "delete_playlist" [ ( "id", Encode.int id ) ]
            )

        StartRenaming id name ->
            -- The button it replaces is gone, so nothing would have focus.
            ( { model | renaming = Just ( id, name ) }
            , Browser.Dom.focus renameFieldId |> Task.attempt (always Focused)
            )

        EditRename name ->
            ( { model | renaming = Maybe.map (\( id, _ ) -> ( id, name )) model.renaming }
            , Cmd.none
            )

        CommitRename ->
            case model.renaming of
                Just ( id, name ) ->
                    ( { model | renaming = Nothing }
                    , invoke "rename_playlist"
                        [ ( "id", Encode.int id ), ( "name", Encode.string name ) ]
                    )

                Nothing ->
                    ( model, Cmd.none )

        CancelRename ->
            ( { model | renaming = Nothing }, Cmd.none )

        SortBy column ->
            ( { model | sort = Playlist.toggleSort column model.sort }, Cmd.none )

        Rate id stars ->
            ( model
            , invoke "set_rating"
                [ ( "id", Encode.int id )
                , ( "stars", Maybe.map Encode.int stars |> Maybe.withDefault Encode.null )
                ]
            )

        PlayFrom id ->
            ( model
            , invoke "play_tracks"
                [ ( "ids", Encode.list Encode.int (visibleIds model) )
                , ( "startId", Encode.int id )
                ]
            )

        Focused ->
            ( model, Cmd.none )

        SetWidth column width ->
            let
                widths : Dict String Int
                widths =
                    Dict.insert (Playlist.columnLabel column) (max 40 width) model.widths
            in
            ( { model | widths = widths }
            , Ports.send (SaveWidths (Dict.toList widths))
            )

        FromJs (Ok (SystemTheme dark)) ->
            ( { model | systemDark = dark }, Cmd.none )

        FromJs (Ok (Event name payload)) ->
            case Library.handleEvent name payload model.library of
                Just result ->
                    updateLibrary model result

                Nothing ->
                    Player.handleEvent name payload model.player
                        |> Maybe.map (updatePlayer model)
                        |> Maybe.withDefault ( model, Cmd.none )

        FromJs (Ok (InvokeResult "ping" (Ok payload))) ->
            ( { model
                | backend =
                    Decode.decodeValue Decode.string payload
                        |> Result.map Connected
                        |> Result.withDefault (Unreachable "unexpected ping payload")
              }
            , Cmd.none
            )

        FromJs (Ok (InvokeResult "ping" (Err error))) ->
            ( { model | backend = Unreachable error }, Cmd.none )

        FromJs (Ok (InvokeResult command outcome)) ->
            case handlePlaylistResult command outcome model of
                Just result ->
                    result

                Nothing ->
                    case Library.handleInvokeResult command outcome model.library of
                        Just result ->
                            updateLibrary model result

                        Nothing ->
                            Player.handleInvokeResult command outcome model.player
                                |> Maybe.map (updatePlayer model)
                                |> Maybe.withDefault ( model, Cmd.none )

        FromJs (Err error) ->
            ( { model | backend = Unreachable (Decode.errorToString error) }, Cmd.none )


invoke : String -> List ( String, Encode.Value ) -> Cmd Msg
invoke name args =
    Ports.send (Invoke name (Encode.object args))


listPlaylists : Cmd Msg
listPlaylists =
    invoke "list_playlists" []


{-| A name for a new playlist that is not taken yet.
-}
nextPlaylistName : List Playlist.Playlist -> String
nextPlaylistName playlists =
    let
        taken : Int -> Bool
        taken n =
            List.any (\playlist -> playlist.name == "Playlist " ++ String.fromInt n) playlists

        firstFree : Int -> Int
        firstFree n =
            if taken n then
                firstFree (n + 1)

            else
                n
    in
    "Playlist " ++ String.fromInt (firstFree 1)


{-| The playlist the tabs are showing, which is the first one until the
user picks another.
-}
activePlaylist : Model -> Maybe Playlist.Playlist
activePlaylist model =
    case model.activePlaylist of
        Just id ->
            List.filter (\playlist -> playlist.id == id) model.playlists |> List.head

        Nothing ->
            List.head model.playlists


{-| The tracks of the open playlist, in the order the table shows them.
-}
visibleTracks : Model -> List Tree.Row
visibleTracks model =
    activePlaylist model
        |> Maybe.map (.tracks >> Playlist.sortBy model.sort)
        |> Maybe.withDefault []


visibleIds : Model -> List Int
visibleIds model =
    List.map .id (visibleTracks model)


updateLibrary : Model -> ( Library.Model, Cmd Library.Msg ) -> ( Model, Cmd Msg )
updateLibrary model ( library, cmd ) =
    ( { model | library = library }, Cmd.map LibraryMsg cmd )


updatePlayer : Model -> ( Player.Model, Cmd Player.Msg ) -> ( Model, Cmd Msg )
updatePlayer model ( player, cmd ) =
    ( { model | player = player }, Cmd.map PlayerMsg cmd )


{-| Handles the replies of the playlist commands. Returns `Nothing` when
the command belongs to another panel.
-}
handlePlaylistResult : String -> Result String Decode.Value -> Model -> Maybe ( Model, Cmd Msg )
handlePlaylistResult command outcome model =
    let
        failed : String -> ( Model, Cmd Msg )
        failed error =
            ( { model | playlistError = Just error }, Cmd.none )

        reload : ( Model, Cmd Msg )
        reload =
            ( { model | playlistError = Nothing }, listPlaylists )
    in
    case ( command, outcome ) of
        ( "list_playlists", Ok payload ) ->
            Just
                (case Decode.decodeValue (Decode.list Playlist.decoder) payload of
                    Ok playlists ->
                        ( { model | playlists = playlists, playlistError = Nothing }, Cmd.none )

                    Err error ->
                        failed (Decode.errorToString error)
                )

        ( "create_playlist", Ok payload ) ->
            Just
                (case Decode.decodeValue Decode.int payload of
                    Ok id ->
                        ( { model | activePlaylist = Just id, playlistError = Nothing }
                        , listPlaylists
                        )

                    Err error ->
                        failed (Decode.errorToString error)
                )

        ( "rename_playlist", Ok _ ) ->
            Just reload

        ( "delete_playlist", Ok _ ) ->
            Just reload

        ( "add_to_playlist", Ok _ ) ->
            Just reload

        ( "set_playlist_tracks", Ok _ ) ->
            Just reload

        ( "set_rating", Ok _ ) ->
            -- The rating went into the file, so the row has to be read
            -- again for the stars to show what was written.
            Just ( { model | playlistError = Nothing }, Cmd.batch [ listPlaylists, Library.refresh |> Cmd.map LibraryMsg ] )

        ( _, Err error ) ->
            if isPlaylistCommand command then
                Just (failed error)

            else
                Nothing

        _ ->
            Nothing


isPlaylistCommand : String -> Bool
isPlaylistCommand command =
    List.member command
        [ "list_playlists"
        , "create_playlist"
        , "rename_playlist"
        , "delete_playlist"
        , "add_to_playlist"
        , "set_playlist_tracks"
        , "set_rating"
        ]


subscriptions : Model -> Sub Msg
subscriptions _ =
    Ports.receive FromJs



-- VIEW


view : Model -> Html Msg
view model =
    let
        mode : Mode
        mode =
            Theme.resolve model.systemDark model.theme
    in
    div [ class "app", attribute "data-theme" (modeAttribute mode) ]
        [ viewSidebar model
        , viewMain model
        , viewPlayerBar model
        ]


modeAttribute : Mode -> String
modeAttribute mode =
    case mode of
        LightMode ->
            "light"

        DarkMode ->
            "dark"


viewSidebar : Model -> Html Msg
viewSidebar model =
    div [ class "sidebar" ]
        [ nav [ class "sidebar-nav" ]
            [ navButton model.panel Library "Library"
            , navButton model.panel NowPlaying "Now playing"
            , navButton model.panel Playlists "Playlists"
            ]
        , div [ class "sidebar-content" ] [ viewPanel model ]
        ]


navButton : Panel -> Panel -> String -> Html Msg
navButton current panel label =
    button
        [ type_ "button"
        , classList [ ( "nav-button", True ), ( "is-active", current == panel ) ]
        , onClick (SelectPanel panel)
        ]
        [ text label ]


viewPanel : Model -> Html Msg
viewPanel model =
    case model.panel of
        Library ->
            -- An Int, not a Maybe: Html.Lazy compares by reference, and a
            -- freshly built `Just` would never match.
            Html.map LibraryMsg
                (Library.view
                    (activePlaylist model |> Maybe.map .id |> Maybe.withDefault 0)
                    model.library
                )

        NowPlaying ->
            placeholder "Now playing" "Cover art and lyrics for the current track."

        Playlists ->
            placeholder "Playlists" "Smart and regular playlists."


placeholder : String -> String -> Html Msg
placeholder heading body =
    div [ class "placeholder" ]
        [ h2 [] [ text heading ]
        , p [] [ text body ]
        ]


viewMain : Model -> Html Msg
viewMain model =
    let
        open : Maybe Playlist.Playlist
        open =
            activePlaylist model
    in
    main_ [ class "main" ]
        [ div [ class "tabs" ]
            (List.map (viewTab model open) model.playlists
                ++ [ button
                        [ type_ "button"
                        , class "tab-add"
                        , title "New playlist"
                        , onClick NewPlaylist
                        ]
                        [ text "+" ]
                   ]
            )
        , case open of
            Nothing ->
                div [ class "main-content" ]
                    [ h1 [] [ text "Satsuma" ]
                    , p [] [ text "Make a playlist, then add tracks to it from the library." ]
                    ]

            Just playlist ->
                viewTable model playlist (visibleTracks model)
        , viewPlaylistError model.playlistError
        ]


viewPlaylistError : Maybe String -> Html Msg
viewPlaylistError error =
    case error of
        Just message ->
            p [ class "error playlist-error" ] [ text message ]

        Nothing ->
            text ""


renameFieldId : String
renameFieldId =
    "playlist-rename"


viewTab : Model -> Maybe Playlist.Playlist -> Playlist.Playlist -> Html Msg
viewTab model open playlist =
    let
        isOpen : Bool
        isOpen =
            Maybe.map .id open == Just playlist.id
    in
    case model.renaming of
        Just ( id, name ) ->
            if id == playlist.id then
                input
                    [ type_ "text"
                    , Attr.id renameFieldId
                    , class "tab-rename"
                    , Attr.value name
                    , Attr.attribute "aria-label" "Playlist name"
                    , onInput EditRename
                    , Html.Events.onBlur CommitRename
                    , onEnterOrEscape CommitRename CancelRename
                    ]
                    []

            else
                tabButton isOpen playlist

        Nothing ->
            tabButton isOpen playlist


tabButton : Bool -> Playlist.Playlist -> Html Msg
tabButton isOpen playlist =
    span [ classList [ ( "tab", True ), ( "is-active", isOpen ) ] ]
        [ button
            [ type_ "button"
            , class "tab-label"
            , title "Double-click to rename"
            , onClick (SelectPlaylist playlist.id)
            , onDoubleClick (StartRenaming playlist.id playlist.name)
            ]
            [ text playlist.name ]
        , button
            [ type_ "button"
            , class "icon-button"
            , title ("Delete " ++ playlist.name)
            , onClick (DeletePlaylist playlist.id)
            ]
            [ text "✕" ]
        ]


{-| Commits on Enter and gives up on Escape, which is what a rename in
place is expected to do.
-}
onEnterOrEscape : Msg -> Msg -> Html.Attribute Msg
onEnterOrEscape commit cancel =
    Html.Events.on "keydown"
        (Decode.field "key" Decode.string
            |> Decode.andThen
                (\key ->
                    case key of
                        "Enter" ->
                            Decode.succeed commit

                        "Escape" ->
                            Decode.succeed cancel

                        _ ->
                            Decode.fail "another key"
                )
        )


viewTable : Model -> Playlist.Playlist -> List Tree.Row -> Html Msg
viewTable model playlist tracks =
    div [ class "playlist" ]
        [ Html.table [ class "playlist-table" ]
            [ Html.thead []
                [ Html.tr [] (List.map (viewHeading model) Playlist.columns) ]
            , Html.tbody []
                (List.map (viewRow model) tracks)
            ]
        , p [ class "playlist-footer" ]
            [ text (Playlist.footerText playlist.tracks) ]
        ]


viewHeading : Model -> Column -> Html Msg
viewHeading model column =
    let
        marker : String
        marker =
            case model.sort of
                Just sort ->
                    if sort.column == column then
                        if sort.ascending then
                            " ▲"

                        else
                            " ▼"

                    else
                        ""

                Nothing ->
                    ""
    in
    Html.th
        [ Attr.style "width" (String.fromInt (widthOf model column) ++ "px")
        , Attr.scope "col"
        ]
        [ button
            [ type_ "button"
            , class "column-heading"
            , onClick (SortBy column)
            ]
            [ text (Playlist.columnLabel column ++ marker) ]
        , span
            [ class "column-grip"
            , title "Drag to resize"
            , Attr.attribute "role" "separator"
            , onResize column (widthOf model column)
            ]
            []
        ]


widthOf : Model -> Column -> Int
widthOf model column =
    Dict.get (Playlist.columnLabel column) model.widths
        |> Maybe.withDefault (Playlist.defaultWidth column)


{-| Resizing follows the pointer: the grip reports where it was dropped and
the column takes the width that leaves.
-}
onResize : Column -> Int -> Html.Attribute Msg
onResize column current =
    Html.Events.on "resized"
        (Decode.at [ "detail", "delta" ] Decode.int
            |> Decode.map (\delta -> SetWidth column (current + delta))
        )


viewRow : Model -> Tree.Row -> Html Msg
viewRow model track =
    let
        isPlaying : Bool
        isPlaying =
            Maybe.map .id model.player.state.track == Just track.id
    in
    Html.tr
        [ classList [ ( "playlist-row", True ), ( "is-playing", isPlaying ) ]
        , onDoubleClick (PlayFrom track.id)
        ]
        (List.map (viewCell track) Playlist.columns)


viewCell : Tree.Row -> Column -> Html Msg
viewCell track column =
    case column of
        Playlist.Rating ->
            Html.td [ class "cell-rating" ] [ viewStars track ]

        _ ->
            let
                content : String
                content =
                    Playlist.cellText column track
            in
            Html.td [ title content ] [ text content ]


{-| Five stars, each of them a button: clicking one rates the track, and
clicking the one already set takes the rating off.
-}
viewStars : Tree.Row -> Html Msg
viewStars track =
    let
        current : Int
        current =
            Maybe.withDefault 0 track.rating

        star : Int -> Html Msg
        star n =
            button
                [ type_ "button"
                , classList [ ( "star", True ), ( "is-set", n <= current ) ]
                , title
                    (if n == current then
                        "Remove the rating"

                     else
                        String.fromInt n
                            ++ (if n == 1 then
                                    " star"

                                else
                                    " stars"
                               )
                    )
                , onClick
                    (Rate track.id
                        (if n == current then
                            Nothing

                         else
                            Just n
                        )
                    )
                ]
                [ text
                    (if n <= current then
                        "★"

                     else
                        "☆"
                    )
                ]
    in
    span [ class "stars" ] (List.map star [ 1, 2, 3, 4, 5 ])


viewPlayerBar : Model -> Html Msg
viewPlayerBar model =
    div [ class "player-bar" ]
        [ Html.map PlayerMsg (Player.view model.player)
        , div [ class "player-bar-end" ]
            [ span [ class "status" ] [ text (backendText model.backend) ]
            , button
                [ type_ "button"
                , class "theme-button"
                , title "Switch theme"
                , onClick CycleTheme
                ]
                [ text ("Theme: " ++ Theme.label model.theme) ]
            ]
        ]


backendText : BackendStatus -> String
backendText status =
    case status of
        Connecting ->
            "Connecting to backend…"

        Connected reply ->
            "Backend: " ++ reply

        Unreachable error ->
            "Backend unreachable: " ++ error
