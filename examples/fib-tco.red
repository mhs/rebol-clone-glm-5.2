fib-tco: func [n acc] [either n <= 0 [acc] [fib-tco n - 1 acc + n]]
print fib-tco 10 0
