## ascii

|                     | `bytes2chars` | `utf8_decode` |   `bstr`   |
| :------------------ | :-----------: | :-----------: | :--------: |
| time (64 KiB)       |   161.65 µs   |   148.68 µs   |  23.59 µs  |
| throughput (64 KiB) | 386.64 MiB/s  | 420.38 MiB/s  | 2.59 GiB/s |

## non_ascii

|                     | `bytes2chars` | `utf8_decode` |   `bstr`   |
| :------------------ | :-----------: | :-----------: | :--------: |
| time (64 KiB)       |   161.02 µs   |   118.35 µs   |  46.19 µs  |
| throughput (64 KiB) | 388.16 MiB/s  | 528.09 MiB/s  | 1.32 GiB/s |
