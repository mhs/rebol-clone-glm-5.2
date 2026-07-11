Red []
; Build/task dialect demo — define and run tasks with dependencies.

build [
    task clean [
        print "Cleaning build artifacts..."
        ; In a real project: delete %target/
    ]

    task compile [
        print "Compiling..."
        ; In a real project: call "cargo build --release"
    ]

    task test [
        print "Running tests..."
        ; In a real project: call "cargo test"
    ]

    task all [
        clean
        compile
        test
    ]

    default all
]

; Run the default task.
; With --build flag, this runs automatically.
; Without --build, use `run` inside the build block or run-task outside.
run-task 'all
