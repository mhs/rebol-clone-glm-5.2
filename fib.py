def fibonacci(n):
    a = 0
    b = 1
    result = []
    for _ in range(n):
        result.append(a)
        a, b = b, a + b
    return result

# Generate first 30 Fibonacci numbers
fib_30 = fibonacci(30)
print(fib_30)

