Red []
; Query dialect demo — query blocks of objects with SQL-like syntax.

people: [
    make object! [name: "Alice" age: 30 city: "NYC"]
    make object! [name: "Bob" age: 25 city: "LA"]
    make object! [name: "Carol" age: 41 city: "NYC"]
    make object! [name: "Dave" age: 35 city: "SF"]
]

; Select all.
print "All people:"
results: query [from people]
print results
print ""

; Filter with WHERE.
print "People over 30:"
print query [from people where [age > 30]]
print ""

; Project specific fields.
print "Names only:"
print query [from people select [name]]
print ""

; Order by age descending.
print "Sorted by age (desc):"
print query [from people order [age desc]]
print ""

; Combined: filter + sort + project + limit.
print "Top 2 oldest in NYC:"
print query [
    from people
    where [city = "NYC"]
    order [age desc]
    select [name age]
    limit 2
]
print ""

; Distinct cities.
print "Distinct cities:"
print query [from people select [city] distinct]
