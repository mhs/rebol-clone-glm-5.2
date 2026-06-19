Red []
; Insertion sort using foreach, forall, and insert
src: [4 2 7 1 3]
sorted: []
foreach n src [
  inserted: false
  forall 'p sorted [
    if not inserted [
      if (first p) > n [
        insert p n
        inserted: true
      ]
    ]
  ]
  if not inserted [append sorted n]
]
print sorted
