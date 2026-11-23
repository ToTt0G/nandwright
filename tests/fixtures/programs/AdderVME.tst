// Runs Adder.vm on the VM emulator.

load Adder.vm,
output-file AdderVME.out,
compare-to AdderVME.cmp,
output-list RAM[0]%D2.6.2 RAM[256]%D2.6.2 RAM[5]%D2.6.2;

set sp 256,
set argument 400,
set argument[0] 20,
set argument[1] 22,

repeat 7 {
    vmstep;
}
output;
