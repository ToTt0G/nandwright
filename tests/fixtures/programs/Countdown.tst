// Runs Countdown.asm on the CPU emulator: counts RAM[0] down to zero,
// writing each value to RAM[100], RAM[101], ...

load Countdown.asm,
output-file Countdown.out,
compare-to Countdown.cmp,
output-list RAM[0]%D2.6.2 RAM[1]%D2.6.2 RAM[100]%D2.6.2 RAM[101]%D2.6.2 RAM[102]%D2.6.2 RAM[103]%D2.6.2;

set RAM[0] 3,
repeat 100 {
    ticktock;
}
output;

set PC 0,
set RAM[0] 1,
repeat 100 {
    ticktock;
}
output;
