Red []
m: module [
    shout: func [s][uppercase s]
    export 'shout]
print m/shout "hello"
