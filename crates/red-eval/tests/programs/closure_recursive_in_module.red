Red []
m: module [
    fact: closure [n][either n <= 1 [1][n * fact n - 1]]
    export 'fact
]
print m/fact 5
