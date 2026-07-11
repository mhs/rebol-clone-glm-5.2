Red []
; HTML builder dialect demo — assemble HTML from tag! literals.
;
; The dialect parses a flat block of tags, strings, and parens.
; Tag open/close structure defines nesting — no nested blocks needed.

; Simple page.
page: html [
    <html>
        <head>
            <title> "My Page" </title>
            <meta charset="utf-8">
        </head>
        <body>
            <h1> "Welcome" </h1>
            <p> "This is a paragraph with " <b> "bold" </b> " text." </p>
            <ul>
                <li> "Item 1" </li>
                <li> "Item 2" </li>
                <li> "Item 3" </li>
            </ul>
            <div>
                <img src="logo.png" alt="Logo">
                <br>
            </div>
            <footer> "© 2026" </footer>
        </body>
    </html>
]

print page

; With paren evaluation in content.
name: "World"
greeting: html [<div class="greeting"> <p> ("Hello, " + name + "!") </p> </div>]
print greeting

; With paren interpolation in attributes.
url: "http://example.com"
link: html [<a href=(url)> "Click here" </a>]
print link

; XML mode (no void elements — all tags need closing).
xml-doc: html/xml [<root> <child> "data" </child> </root>]
print xml-doc
