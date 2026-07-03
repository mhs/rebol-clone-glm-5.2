Red []
p: open %tests/fixtures/_tmp_port_out.txt
write p "hi"
close p
print read %tests/fixtures/_tmp_port_out.txt
delete %tests/fixtures/_tmp_port_out.txt
