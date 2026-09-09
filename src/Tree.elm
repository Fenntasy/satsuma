module Tree exposing
    ( Children(..)
    , Grouping(..)
    , Node
    , Row
    , build
    , buildSorted
    , groupingLabel
    , groupings
    , rowDecoder
    , sortFor
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
    , discNumber : Maybe Int
    , rating : Maybe Int
    , grouping : Maybe String
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

    -- Kept rather than walked on demand: the view needs them for every
    -- visible row, and walking would cover the whole library each render.
    , ids : List Int
    }


type Children
    = Branches (List Node)
    | Track


{-| Builds the tree, keeping only the rows that match `filter`.

Sorting is the expensive half and does not depend on the filter, so a
caller that filters often sorts once with [`sortFor`] and then calls
[`buildSorted`].

-}
build : Grouping -> String -> List Row -> List Node
build grouping filter rows =
    buildSorted grouping filter (sortFor grouping rows)


{-| As [`build`], for rows already ordered by [`sortFor`] for this same
grouping. Filtering keeps the order it was given.
-}
buildSorted : Grouping -> String -> List Row -> List Node
buildSorted grouping filter rows =
    let
        kept : List Row
        kept =
            if String.isEmpty (String.trim filter) then
                rows

            else
                List.filter (matches filter) rows
    in
    group (levels grouping) (groupingKey grouping) kept


{-| The start of every path, so what is expanded under one grouping does
not pre-expand an unrelated node under another.
-}
groupingKey : Grouping -> String
groupingKey grouping =
    case grouping of
        GenreArtistAlbum ->
            "genre"

        ArtistAlbum ->
            "artist"

        AlbumOnly ->
            "album"


{-| A number as a string that sorts the way the number does.
-}
sortable : Maybe Int -> String
sortable value =
    String.padLeft 6 '0' (String.fromInt (Maybe.withDefault 0 value))


{-| Separates the parts of a path. A tag can contain a slash, so the
separator has to be something a tag cannot hold.
-}
separator : String
separator =
    "\u{001F}"


{-| Orders the rows by the levels they will be grouped under, then by disc,
track and title.

The database sorts by genre first, so grouping by artist alone would
otherwise meet the same artist twice and give it two branches. The sort is
stable, so the track order the database chose survives inside each album.

-}
sortFor : Grouping -> List Row -> List Row
sortFor grouping rows =
    let
        key : Row -> List String
        key row =
            List.map (\level -> String.toLower (Maybe.withDefault unknown (level row)))
                (levels grouping)
                -- Then disc, track and title, so an album keeps its order
                -- whichever levels the grouping happens to use.
                ++ [ sortable row.discNumber, sortable row.trackNumber ]
                ++ [ String.toLower (Maybe.withDefault "" row.title) ]
    in
    -- The key is built once per row rather than once per comparison: it
    -- lowercases up to three fields and allocates a list.
    rows
        |> List.map (\row -> ( key row, row ))
        |> List.sortBy Tuple.first
        |> List.map Tuple.second


{-| Whether a row matches what was typed, in any of the fields the tree
shows. Case and surrounding spaces are ignored.
-}
matches : String -> Row -> Bool
matches filter row =
    let
        needle : String
        needle =
            String.toLower (String.trim filter)
    in
    -- Each field on its own: joining them would match across their
    -- boundaries, so "alpha first" would find artist Alpha next to album
    -- First and the hit would be impossible to explain.
    [ row.genre, row.artist, row.album, row.title ]
        |> List.filterMap identity
        |> List.any (\field -> String.contains needle (String.toLower field))


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
                                prefix ++ separator ++ label

                            children : List Node
                            children =
                                group deeper path inside
                        in
                        { path = path
                        , label = label
                        , children = Branches children
                        , trackCount = List.length inside
                        , durationMs = List.sum (List.map .durationMs inside)
                        , ids = List.concatMap .ids children
                        }
                    )


leaf : String -> Row -> Node
leaf prefix row =
    { path = prefix ++ separator ++ String.fromInt row.id
    , label = trackLabel row
    , children = Track
    , trackCount = 1
    , durationMs = row.durationMs
    , ids = [ row.id ]
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
            discPrefix row ++ String.fromInt number ++ ". " ++ title

        Nothing ->
            title


{-| The disc is shown only past the first, so a two-disc album does not
repeat "1." and "2." with nothing to tell them apart.
-}
discPrefix : Row -> String
discPrefix row =
    case row.discNumber of
        Just number ->
            if number > 1 then
                String.fromInt number ++ "."

            else
                ""

        Nothing ->
            ""


unknown : String
unknown =
    "Unknown"


{-| Groups rows whose value matches once case is ignored, which is how they
were sorted: grouping on the raw value would split "Alpha" from "ALPHA"
into two branches that then share one path.

Each group is labelled with the first spelling it met, and only rows that
are next to each other are merged, which the sort guarantees.

-}
groupBy : (a -> String) -> List a -> List ( String, List a )
groupBy value items =
    let
        key : a -> String
        key =
            value >> String.toLower
    in
    List.foldr
        (\item acc ->
            case acc of
                ( label, group_ ) :: rest ->
                    if key item == String.toLower label then
                        -- The first spelling wins, and folding from the
                        -- right means the first row is met last.
                        ( value item, item :: group_ ) :: rest

                    else
                        ( value item, [ item ] ) :: acc

                [] ->
                    [ ( value item, [ item ] ) ]
        )
        []
        items



-- DECODERS


rowDecoder : Decoder Row
rowDecoder =
    Decode.map8 Row
        (Decode.field "id" Decode.int)
        (Decode.field "genre" (Decode.nullable Decode.string))
        (Decode.field "artist" (Decode.nullable Decode.string))
        (Decode.field "album" (Decode.nullable Decode.string))
        (Decode.field "title" (Decode.nullable Decode.string))
        (Decode.field "track_number" (Decode.nullable Decode.int))
        (Decode.field "disc_number" (Decode.nullable Decode.int))
        (Decode.field "rating" (Decode.nullable Decode.int))
        |> andMap (Decode.field "grouping" (Decode.nullable Decode.string))
        |> andMap (Decode.field "duration_ms" Decode.int)


{-| Applies one more field to a decoder that is still a function, which is
how a record with more than eight fields is decoded.
-}
andMap : Decoder a -> Decoder (a -> b) -> Decoder b
andMap =
    Decode.map2 (|>)
