Red []
; HTML builder dialect demo — assemble HTML from blocks.

; Simple page.
page: html [
    html [
        head [
            title "My Page"
            meta charset "utf-8"
        ]
        body [
            h1 "Welcome"
            p "This is a paragraph with " [b "bold"] " text."
            ul [
                li "Item 1"
                li "Item 2"
                li "Item 3"
            ]
            div [
                img src "logo.png" alt "Logo"
                br
            ]
            footer "© 2026"
        ]
    ]
]

print page

; With paren evaluation.
name: "World"
greeting: html [div class "greeting" [p ("Hello, " + name + "!")]]
print greeting

; XML mode.
xml-doc: html/xml [root [child "data"]]
print xml-doc
