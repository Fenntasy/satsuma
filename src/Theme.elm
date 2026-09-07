module Theme exposing (Mode(..), Setting(..), cycle, decoder, fromString, label, resolve, toString)

{-| Light/dark theme handling. `Setting` is what the user chose; `Mode` is what
is actually rendered once the OS preference is taken into account.
-}

import Json.Decode as Decode exposing (Decoder)


type Setting
    = System
    | Light
    | Dark


type Mode
    = LightMode
    | DarkMode


resolve : Bool -> Setting -> Mode
resolve systemPrefersDark setting =
    case setting of
        System ->
            if systemPrefersDark then
                DarkMode

            else
                LightMode

        Light ->
            LightMode

        Dark ->
            DarkMode


cycle : Setting -> Setting
cycle setting =
    case setting of
        System ->
            Light

        Light ->
            Dark

        Dark ->
            System


label : Setting -> String
label setting =
    case setting of
        System ->
            "System"

        Light ->
            "Light"

        Dark ->
            "Dark"


toString : Setting -> String
toString setting =
    case setting of
        System ->
            "system"

        Light ->
            "light"

        Dark ->
            "dark"


fromString : String -> Maybe Setting
fromString raw =
    case raw of
        "system" ->
            Just System

        "light" ->
            Just Light

        "dark" ->
            Just Dark

        _ ->
            Nothing


decoder : Decoder Setting
decoder =
    Decode.string
        |> Decode.andThen
            (\raw ->
                case fromString raw of
                    Just setting ->
                        Decode.succeed setting

                    Nothing ->
                        Decode.fail ("Unknown theme setting: " ++ raw)
            )
