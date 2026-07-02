Red []
module 'lib [a: 7 export 'a]
import 'lib
f: closure [x][x + a]
print f 3
a: 100
print f 3
