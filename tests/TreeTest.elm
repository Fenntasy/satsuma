module TreeTest exposing (suite)

import Expect
import Test exposing (Test, describe, test)
import Tree exposing (Children(..), Grouping(..))


suite : Test
suite =
    describe "Tree"
        [ describe "build"
            [ test "groups by genre, then artist, then album" <|
                \_ ->
                    Tree.build GenreArtistAlbum "" library
                        |> labels
                        |> Expect.equal [ "Indie", "Rock" ]
            , test "the second level is the artists of that genre" <|
                \_ ->
                    Tree.build GenreArtistAlbum "" library
                        |> childrenOf "Indie"
                        |> labels
                        |> Expect.equal [ "Alpha", "Beta" ]
            , test "an album with two artists appears under each of them" <|
                \_ ->
                    -- No "various artists" bucket: the tags decide.
                    Tree.build ArtistAlbum "" library
                        |> childrenOf "Alpha"
                        |> labels
                        |> Expect.equal [ "First", "Split" ]
            , test "tracks are the last level, numbered" <|
                \_ ->
                    Tree.build ArtistAlbum "" library
                        |> childrenOf "Alpha"
                        |> childrenOf "First"
                        |> labels
                        |> Expect.equal [ "1. One", "2. Two" ]
            , test "an album keeps its track order whatever the grouping" <|
                \_ ->
                    -- The database sorts by artist before track, so an
                    -- album whose tracks name different artists comes back
                    -- out of order unless the tree sorts it again.
                    let
                        split : List Tree.Row
                        split =
                            [ row 2 "Indie" "Alpha" "Split" "Second" (Just 2)
                            , row 1 "Indie" "Zed" "Split" "First" (Just 1)
                            ]
                    in
                    Tree.build AlbumOnly "" split
                        |> childrenOf "Split"
                        |> labels
                        |> Expect.equal [ "1. First", "2. Second" ]
            , test "the ids of a branch follow the order it shows" <|
                \_ ->
                    let
                        split : List Tree.Row
                        split =
                            [ row 2 "Indie" "Alpha" "Split" "Second" (Just 2)
                            , row 1 "Indie" "Zed" "Split" "First" (Just 1)
                            ]
                    in
                    Tree.build AlbumOnly "" split
                        |> List.concatMap .ids
                        |> Expect.equal [ 1, 2 ]
            , test "tags that differ only in case are one branch" <|
                \_ ->
                    let
                        shouty : List Tree.Row
                        shouty =
                            [ row 1 "Indie" "Alpha" "First" "One" (Just 1)
                            , row 2 "Indie" "ALPHA" "First" "Two" (Just 2)
                            , row 3 "Indie" "Alpha" "First" "Three" (Just 3)
                            ]
                    in
                    Tree.build ArtistAlbum "" shouty
                        |> labels
                        |> Expect.equal [ "Alpha" ]
            , test "grouping by album only skips the levels above" <|
                \_ ->
                    Tree.build AlbumOnly "" library
                        |> labels
                        |> Expect.equal [ "First", "Second", "Split" ]
            , test "a missing tag becomes Unknown rather than being dropped" <|
                \_ ->
                    Tree.build GenreArtistAlbum "" [ untagged ]
                        |> labels
                        |> Expect.equal [ "Unknown" ]
            , test "a branch carries how many tracks are under it" <|
                \_ ->
                    Tree.build GenreArtistAlbum "" library
                        |> List.map (\node -> ( node.label, node.trackCount ))
                        |> Expect.equal [ ( "Indie", 3 ), ( "Rock", 1 ) ]
            , test "a branch carries the total duration under it" <|
                \_ ->
                    Tree.build GenreArtistAlbum "" library
                        |> List.map .durationMs
                        |> Expect.equal [ 3000, 1000 ]
            , test "an empty library is an empty tree" <|
                \_ -> Tree.build GenreArtistAlbum "" [] |> Expect.equal []
            ]
        , describe "filtering"
            [ test "keeps only what matches, in any field" <|
                \_ ->
                    Tree.build ArtistAlbum "beta" library
                        |> labels
                        |> Expect.equal [ "Beta" ]
            , test "does not match across two fields" <|
                \_ ->
                    -- "Alpha" is the artist and "First" the album: joining
                    -- the fields would make this a hit no one can explain.
                    Tree.build ArtistAlbum "alpha first" library
                        |> Expect.equal []
            , test "matches a title as well as a name" <|
                \_ ->
                    Tree.build ArtistAlbum "three" library
                        |> childrenOf "Beta"
                        |> labels
                        |> Expect.equal [ "Second" ]
            , test "ignores case and surrounding spaces" <|
                \_ ->
                    Tree.build ArtistAlbum "  ALPHA " library
                        |> labels
                        |> Expect.equal [ "Alpha" ]
            , test "an empty filter keeps everything" <|
                \_ ->
                    Tree.build ArtistAlbum "   " library
                        |> labels
                        |> Expect.equal [ "Alpha", "Beta" ]
            , test "a filter that matches nothing gives an empty tree" <|
                \_ ->
                    Tree.build ArtistAlbum "nothing here" library
                        |> Expect.equal []
            ]
        , describe "idsOf"
            [ test "a branch answers with every track under it, in order" <|
                \_ ->
                    Tree.build GenreArtistAlbum "" library
                        |> List.concatMap .ids
                        |> Expect.equal [ 1, 2, 3, 4 ]
            , test "a track answers with itself" <|
                \_ ->
                    Tree.build ArtistAlbum "" library
                        |> childrenOf "Alpha"
                        |> childrenOf "First"
                        |> List.concatMap .ids
                        |> Expect.equal [ 1, 2 ]
            ]
        , describe "paths"
            [ test "a tag holding a slash cannot collide with a deeper node" <|
                \_ ->
                    let
                        awkward : List Tree.Row
                        awkward =
                            [ row 1 "Indie/Rock" "Someone" "An album" "One" (Just 1)
                            , row 2 "Indie" "Rock" "Another" "Two" (Just 1)
                            ]
                    in
                    Tree.build GenreArtistAlbum "" awkward
                        |> List.concatMap
                            (\node ->
                                case node.children of
                                    Branches children ->
                                        List.map .path children

                                    Track ->
                                        []
                            )
                        |> uniqueCount
                        |> Expect.equal 2
            , test "a track on a second disc says which disc it is on" <|
                \_ ->
                    Tree.build AlbumOnly "" [ secondDisc ]
                        |> childrenOf "An album"
                        |> labels
                        |> Expect.equal [ "2.5. Deep cut" ]
            , test "identify a node so what is open stays open" <|
                \_ ->
                    Tree.build GenreArtistAlbum "" library
                        |> List.map .path
                        |> Expect.equal [ "genre\u{001F}Indie", "genre\u{001F}Rock" ]
            , test "are unique per level" <|
                \_ ->
                    Tree.build GenreArtistAlbum "" library
                        |> childrenOf "Indie"
                        |> List.map .path
                        |> Expect.equal
                            [ "genre\u{001F}Indie\u{001F}Alpha"
                            , "genre\u{001F}Indie\u{001F}Beta"
                            ]
            ]
        ]


{-| A small library, already sorted the way the database returns it.
-}
library : List Tree.Row
library =
    [ row 1 "Indie" "Alpha" "First" "One" (Just 1)
    , row 2 "Indie" "Alpha" "First" "Two" (Just 2)
    , row 3 "Indie" "Beta" "Second" "Three" (Just 1)
    , row 4 "Rock" "Alpha" "Split" "Four" Nothing
    ]


untagged : Tree.Row
untagged =
    { id = 9
    , genre = Nothing
    , artist = Nothing
    , album = Nothing
    , title = Nothing
    , trackNumber = Nothing
    , discNumber = Nothing
    , rating = Nothing
    , grouping = Nothing
    , durationMs = 1000
    }


row : Int -> String -> String -> String -> String -> Maybe Int -> Tree.Row
row id genre artist album title trackNumber =
    { id = id
    , genre = Just genre
    , artist = Just artist
    , album = Just album
    , title = Just title
    , trackNumber = trackNumber
    , discNumber = Just 1
    , rating = Nothing
    , grouping = Nothing
    , durationMs = 1000
    }


secondDisc : Tree.Row
secondDisc =
    { id = 7
    , genre = Just "Indie"
    , artist = Just "Someone"
    , album = Just "An album"
    , title = Just "Deep cut"
    , trackNumber = Just 5
    , discNumber = Just 2
    , rating = Nothing
    , grouping = Nothing
    , durationMs = 1000
    }


uniqueCount : List String -> Int
uniqueCount paths =
    paths
        |> List.foldl
            (\path seen ->
                if List.member path seen then
                    seen

                else
                    path :: seen
            )
            []
        |> List.length


labels : List Tree.Node -> List String
labels =
    List.map .label


{-| The children of the node with this label, or nothing when it has none.
-}
childrenOf : String -> List Tree.Node -> List Tree.Node
childrenOf label nodes =
    nodes
        |> List.filter (\node -> node.label == label)
        |> List.concatMap
            (\node ->
                case node.children of
                    Branches children ->
                        children

                    Track ->
                        []
            )
