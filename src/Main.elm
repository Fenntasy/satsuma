module Main exposing (BackendStatus, Flags, Model, Msg, Panel, main)

import Bridge exposing (Incoming(..), Outgoing(..))
import Browser
import Html exposing (Html, button, div, h1, h2, main_, nav, p, span, text)
import Html.Attributes exposing (attribute, class, classList, title, type_)
import Html.Events exposing (onClick)
import Json.Decode as Decode
import Json.Encode as Encode
import Library
import Player
import Ports
import Theme exposing (Mode(..), Setting(..))


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
      }
    , Cmd.batch
        [ Ports.send (Invoke "ping" (Encode.object []))
        , Cmd.map LibraryMsg libraryCmd
        , Cmd.map PlayerMsg playerCmd
        ]
    )



-- UPDATE


type Msg
    = SelectPanel Panel
    | CycleTheme
    | LibraryMsg Library.Msg
    | PlayerMsg Player.Msg
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
            case Library.handleInvokeResult command outcome model.library of
                Just result ->
                    updateLibrary model result

                Nothing ->
                    Player.handleInvokeResult command outcome model.player
                        |> Maybe.map (updatePlayer model)
                        |> Maybe.withDefault ( model, Cmd.none )

        FromJs (Err error) ->
            ( { model | backend = Unreachable (Decode.errorToString error) }, Cmd.none )


updateLibrary : Model -> ( Library.Model, Cmd Library.Msg ) -> ( Model, Cmd Msg )
updateLibrary model ( library, cmd ) =
    ( { model | library = library }, Cmd.map LibraryMsg cmd )


updatePlayer : Model -> ( Player.Model, Cmd Player.Msg ) -> ( Model, Cmd Msg )
updatePlayer model ( player, cmd ) =
    ( { model | player = player }, Cmd.map PlayerMsg cmd )


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
        , viewMain
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
            Html.map LibraryMsg (Library.view model.library)

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


viewMain : Html Msg
viewMain =
    main_ [ class "main" ]
        [ div [ class "tabs" ]
            [ span [ class "tab is-active" ] [ text "Playlist 1" ] ]
        , div [ class "main-content" ]
            [ h1 [] [ text "Satsuma" ]
            , p [] [ text "Open a playlist or add tracks from the library." ]
            ]
        ]


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
