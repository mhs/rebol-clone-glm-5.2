Red []
b: read/binary %tests/fixtures/hello.txt
print b
print length? b
print pick b 1
write/binary %tests/fixtures/_tmp_out.bin #{48656C6C6F}
print read/binary %tests/fixtures/_tmp_out.bin
delete %tests/fixtures/_tmp_out.bin
