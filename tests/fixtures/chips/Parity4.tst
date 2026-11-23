load Parity4.hdl,
output-file Parity4.out,
compare-to Parity4.cmp,
output-list in%B1.4.1 out%B2.1.2;

set in %B0000, eval, output;
set in %B0001, eval, output;
set in %B0011, eval, output;
set in %B0111, eval, output;
set in %B1111, eval, output;
set in %B1010, eval, output;
set in %B1000, eval, output;
set in %B1101, eval, output;
