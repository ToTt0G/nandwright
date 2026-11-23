// A script for a chip that is not implemented yet must not be full of
// errors: RAM8 and ROM32K will exist once the chip is written.

load Unbuilt.hdl,
output-file Unbuilt.out,
output-list in%D1.6.1 out%D1.6.1 RAM8[3]%D1.6.1;

ROM32K load Program.hack,
set RAM8[3] 7,
set in 5, tick, tock, output;
