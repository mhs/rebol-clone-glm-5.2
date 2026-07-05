Red []
print checksum "123456789"
print checksum/method "hello" 'sha256
c: compress "hello world hello world hello world"
d: decompress c
print d
print enbase "hello"
print debase "aGVsbG8="
print encode 'url "a b c"
print decode 'url "a%20b%20c"
