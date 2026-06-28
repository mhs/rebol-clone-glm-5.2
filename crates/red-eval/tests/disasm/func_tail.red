Red []
fact-tail: func [n acc] [ either n <= 1 [acc] [fact-tail n - 1 n * acc] ]
