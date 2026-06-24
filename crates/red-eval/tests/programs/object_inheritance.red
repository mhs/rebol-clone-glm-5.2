Red []
animal: make object! [
    legs: 4
    sound: "generic"
    describe: does [
        rejoin ["Animal with " legs " legs says " sound]
    ]
]

dog: make object! [animal sound: "Woof"]
cat: make object! [animal sound: "Meow" legs: 4]

print dog/describe
print cat/describe
print dog/sound
print dog/legs
