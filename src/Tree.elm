module Tree exposing
    ( Children(..)
    , Grouping(..)
    , Node
    , Row
    , build
    , groupingLabel
    , groupings
    , idsOf
    , rowDecoder
    )

{-| Turning the library into the tree the left panel shows.

Tracks are grouped strictly by the tags they carry: an album whose tracks
name different artists appears under each of them. There is no "various
artists" bucket, ever.

This module is pure, so every grouping and filtering rule is tested
directly.

-}

import Json.Decode as Decode exposing (Decoder)


type alias Row =
    { id : Int
    , genre : Maybe String
    , artist : Maybe String
    , album : Maybe String
    , title : Maybe String
    , trackNumber : Maybe Int
    , durationMs : Int
    }


{-| How the tree is grouped. The last level is always the tracks.
-}
type Grouping
    = GenreArtistAlbum
    | ArtistAlbum
    | AlbumOnly


groupings : List Grouping
groupings =
    [ GenreArtistAlbum, ArtistAlbum, AlbumOnly ]


groupingLabel : Grouping -> String
groupingLabel grouping =
    case grouping of
        GenreArtistAlbum ->
            "Genre / Artist / Album"

        ArtistAlbum ->
            "Artist / Album"

        AlbumOnly ->
            "Album"


{-| A branch of the tree, or a track at its tip.

`path` identifies the node across rebuilds, so what the user expanded stays
expanded when the library is rescanned.

-}
type alias Node =
    { path : String
    , label : String
    , children : Children
    , trackCount : Int
    , durationMs : Int
    }


type Children
    = Branches (List Node)
    | Track Int


{-| The tracks under a node, in the order they are shown.
-}
idsOf : Node -> List Int
idsOf node =
    case node.children of
        Track id ->
            [ id ]

        Branches children ->
            List.concatMap idsOf children


{-| Builds the tree, keeping only the rows that match `filter`.
-}
build : Grouping -> String -> List Row -> List Node
build grouping filter rows =
    let
        kept : List Row
        kept =
            if String.isEmpty (String.trim filter) then
                rows

            else
                List.filter (matches filter) rows
    in
    kept
        |> sortFor grouping
        |> group (levels grouping) ""


{-| Orders the rows by the levels they will be grouped under.

The database sorts by genre first, so grouping by artist alone would
otherwise meet the same artist twice and give it two branches. The sort is
stable, so the track order the database chose survives inside each album.

-}
sortFor : Grouping -> List Row -> List Row
sortFor grouping rows =
    let
        key : Row -> List String
        key row =
            levels grouping
                |> List.map (\level -> String.toLower (Maybe.withDefault unknown (level row)))
    in
    List.sortBy key rows


{-| Whether a row matches what was typed, in any of the fields the tree
shows. Case and surrounding spaces are ignored.
-}
matches : String -> Row -> Bool
matches filter row =
    let
        needle : String
        needle =
            String.toLower (String.trim filter)

        haystack : String
        haystack =
            [ row.genre, row.artist, row.album, row.title ]
                |> List.filterMap identity
                |> String.join " "
                |> String.toLower
    in
    String.contains needle haystack


{-| The fields to group by, outermost first.
-}
levels : Grouping -> List (Row -> Maybe String)
levels grouping =
    case grouping of
        GenreArtistAlbum ->
            [ .genre, .artist, .album ]

        ArtistAlbum ->
            [ .artist, .album ]

        AlbumOnly ->
            [ .album ]


group : List (Row -> Maybe String) -> String -> List Row -> List Node
group remaining prefix rows =
    case remaining of
        [] ->
            List.map (leaf prefix) rows

        level :: deeper ->
            rows
                |> groupBy (level >> Maybe.withDefault unknown)
                |> List.map
                    (\( label, inside ) ->
                        let
                            path : String
                            path =
                                prefix ++ "/" ++ label

                            children : List Node
                            children =
                                group deeper path inside
                        in
                        { path = path
                        , label = label
                        , children = Branches children
                        , trackCount = List.length inside
                        , durationMs = List.sum (List.map .durationMs inside)
                        }
                    )


leaf : String -> Row -> Node
leaf prefix row =
    { path = prefix ++ "/" ++ String.fromInt row.id
    , label = trackLabel row
    , children = Track row.id
    , trackCount = 1
    , durationMs = row.durationMs
    }


trackLabel : Row -> String
trackLabel row =
    let
        title : String
        title =
            Maybe.withDefault "Unknown title" row.title
    in
    case row.trackNumber of
        Just number ->
            String.fromInt number ++ ". " ++ title

        Nothing ->
            title


unknown : String
unknown =
    "Unknown"


{-| Groups consecutive-equal keys, keeping the order the rows came in: the
database already sorted them.
-}
groupBy : (a -> String) -> List a -> List ( String, List a )
groupBy key items =
    List.foldr
        (\item acc ->
            case acc of
                ( previous, group_ ) :: rest ->
                    if previous == key item then
                        ( previous, item :: group_ ) :: rest

                    else
                        ( key item, [ item ] ) :: acc

                [] ->
                    [ ( key item, [ item ] ) ]
        )
        []
        items



-- DECODERS


rowDecoder : Decoder Row
rowDecoder =
    Decode.map7 Row
        (Decode.field "id" Decode.int)
        (Decode.field "genre" (Decode.nullable Decode.string))
        (Decode.field "artist" (Decode.nullable Decode.string))
        (Decode.field "album" (Decode.nullable Decode.string))
        (Decode.field "title" (Decode.nullable Decode.string))
        (Decode.field "track_number" (Decode.nullable Decode.int))
        (Decode.field "duration_ms" Decode.int)
