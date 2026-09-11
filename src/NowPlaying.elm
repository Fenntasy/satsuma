module NowPlaying exposing
    ( Details
    , Model
    , coverUrl
    , decoder
    , handleInvokeResult
    , init
    , loading
    , trackChanged
    , view
    )

{-| The panel that shows what is playing: its cover, and what it is.

The cover is not carried here. Elm holds the key the backend issued and
puts it in a URL; the bytes travel over the `satsuma-cover` protocol,
where the webview keeps them rather than fetching one per track of the
same record.

-}

import Html exposing (Html, div, dl, h2, img, p, span, text)
import Html.Attributes exposing (alt, class, src)
import Json.Decode as Decode exposing (Decoder)


{-| What the backend says about one track.
-}
type alias Details =
    { id : Int
    , title : Maybe String
    , artist : Maybe String
    , albumArtist : Maybe String
    , album : Maybe String
    , genre : Maybe String
    , year : Maybe Int
    , rating : Maybe Int
    , grouping : Maybe String
    , coverKey : String
    }


{-| What the panel is showing.

One field rather than a pair of a `Maybe Details` and a `Maybe String`:
those two could disagree, and there is no state where the panel shows a
record and a failure to read it at the same time.

-}
type Model
    = Empty
    | Loading Int
    | Showing Details
    | Gone Int
    | Failed Int String


init : Model
init =
    Empty


{-| The panel has asked about this track and is waiting.

Held so that the player state, which arrives five times a second, does not
ask again for every one of them until the first answer lands.

-}
loading : Int -> Model
loading =
    Loading


decoder : Decoder Details
decoder =
    Decode.map8 Details
        (Decode.field "id" Decode.int)
        (nullable "title" Decode.string)
        (nullable "artist" Decode.string)
        (nullable "album_artist" Decode.string)
        (nullable "album" Decode.string)
        (nullable "genre" Decode.string)
        (nullable "year" Decode.int)
        (nullable "rating" Decode.int)
        |> andMap (nullable "grouping" Decode.string)
        |> andMap (Decode.field "cover_key" Decode.string)


{-| A field the backend always sends and may set to null. Not
`Decode.maybe`, which also answers `Nothing` for a field that is missing
or holds the wrong type, and so would hide a rename on either side.
-}
nullable : String -> Decoder a -> Decoder (Maybe a)
nullable field inner =
    Decode.field field (Decode.nullable inner)


{-| `map8` is as wide as the decoders go, so the rest are applied one at a
time.
-}
andMap : Decoder a -> Decoder (a -> b) -> Decoder b
andMap =
    Decode.map2 (|>)


{-| Handles the reply to `track_details`. `Nothing` when the command
belongs to another panel.

A track that has left the library answers with nothing at all. Which
track that was is kept even so, because the player goes on naming it and
every state event would otherwise ask about it again; the panel says the
track is no longer in the library rather than claiming nothing is
playing.

-}
handleInvokeResult : String -> Result String Decode.Value -> Model -> Maybe Model
handleInvokeResult command outcome model =
    if command /= "track_details" then
        Nothing

    else
        let
            -- A failure keeps the track it was about, so the panel does
            -- not ask about it again on the next state event, and starts
            -- afresh when the track changes.
            failed : String -> Model
            failed error =
                current model
                    |> Maybe.map (\id -> Failed id error)
                    |> Maybe.withDefault Empty
        in
        Just
            (case outcome of
                Ok payload ->
                    case Decode.decodeValue (Decode.nullable decoder) payload of
                        Ok (Just details) ->
                            Showing details

                        Ok Nothing ->
                            -- The track kept, not forgotten: the player
                            -- holds its own queue and goes on reporting a
                            -- track whose file was deleted and whose row
                            -- the next scan removed. Forgetting it here
                            -- made every state event ask again, five
                            -- times a second, for as long as it played.
                            current model
                                |> Maybe.map Gone
                                |> Maybe.withDefault Empty

                        Err error ->
                            -- Said rather than swallowed: a field renamed
                            -- on one side would otherwise look exactly
                            -- like nothing playing.
                            failed (Decode.errorToString error)

                Err error ->
                    failed error
            )


{-| Whether the details held are still the ones for this track.

The player reports its state five times a second and the track within it
changes only now and then, so this is what stops the panel asking for
details it already has.

-}
trackChanged : Maybe Int -> Model -> Bool
trackChanged playing model =
    playing /= current model


{-| The track the panel is about, whether it is showing it, waiting for
it or failed to read it.
-}
current : Model -> Maybe Int
current model =
    case model of
        Empty ->
            Nothing

        Loading id ->
            Just id

        Showing details ->
            Just details.id

        Gone id ->
            Just id

        Failed id _ ->
            Just id


{-| Where the webview fetches the cover of these details.

`base` is the shape of a custom-protocol URL on this platform, which
differs between Windows and everywhere else and so is worked out by the
bridge at startup rather than guessed at here.

-}
coverUrl : String -> Details -> String
coverUrl base details =
    base ++ details.coverKey


view : String -> Model -> Html msg
view coverBase model =
    case model of
        Empty ->
            div [ class "now-playing is-empty" ]
                [ p [] [ text "Nothing is playing." ] ]

        Loading _ ->
            div [ class "now-playing is-empty" ] []

        Gone _ ->
            div [ class "now-playing is-empty" ]
                [ p [] [ text "This track is no longer in the library." ] ]

        Failed _ error ->
            div [ class "now-playing is-empty" ]
                [ p [ class "error" ] [ text ("Cannot read the track: " ++ error) ] ]

        Showing details ->
            div [ class "now-playing" ]
                [ div [ class "cover" ]
                    [ img
                        [ src (coverUrl coverBase details)
                        , alt (Maybe.withDefault "Cover" details.album)
                        , class "cover-art"
                        ]
                        []
                    ]
                , div [ class "track-info" ]
                    [ h2 [] [ text (Maybe.withDefault "Unknown title" details.title) ]
                    , p [ class "by" ] [ text (byLine details) ]
                    , dl [ class "tags" ] (List.concatMap tag (facts details))
                    ]
                ]


{-| Who the record is by. The album artist is shown alongside the
performer when they differ, because that difference is the whole reason a
compilation holds together.
-}
byLine : Details -> String
byLine details =
    case ( details.artist, details.albumArtist ) of
        ( Just artist, Just albumArtist ) ->
            if artist == albumArtist then
                artist

            else
                artist ++ " — " ++ albumArtist

        ( Just artist, Nothing ) ->
            artist

        ( Nothing, Just albumArtist ) ->
            albumArtist

        ( Nothing, Nothing ) ->
            "Unknown artist"


{-| The tags worth showing, with the ones the file does not carry left
out rather than shown as empty.
-}
facts : Details -> List ( String, String )
facts details =
    List.filterMap identity
        [ Maybe.map (Tuple.pair "Album") details.album
        , Maybe.map (Tuple.pair "Year" << String.fromInt) details.year
        , Maybe.map (Tuple.pair "Genre") details.genre
        , Maybe.map (Tuple.pair "Grouping") details.grouping
        , Maybe.map (Tuple.pair "Rating" << stars) details.rating
        ]


stars : Int -> String
stars rating =
    String.repeat (clamp 0 5 rating) "★" ++ String.repeat (5 - clamp 0 5 rating) "☆"


tag : ( String, String ) -> List (Html msg)
tag ( label, value ) =
    [ Html.dt [] [ text label ]
    , Html.dd [] [ span [] [ text value ] ]
    ]
