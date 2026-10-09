# cdd_code_gen

**English** · [한국어](#한국어)

---

# English

## Contents

1. [What it is](#1-what-it-is)
2. [Install](#2-install)
3. [A quick tour of a CDD](#3-a-quick-tour-of-a-cdd)
4. [Generate code](#4-generate-code)
5. [Use the generated code](#5-use-the-generated-code)
6. [Reference](#6-reference)
7. [Good to know](#7-good-to-know)
8. [Troubleshooting](#8-troubleshooting)
9. [Limitations](#9-limitations)
10. [Verification and development](#10-verification-and-development)
11. [Status and license](#11-status-and-license)

## 1. What it is

`cddgen` reads a **CDD** file (the diagnostic description CANdela / CANdelaStudio writes) and generates **Rust code** for the
UDS or KWP2000 services in it. The generated code lets you:

- **Test an ECU** the way CANoe does: send a request, get a typed response, check values, names and units, and see the
  negative response code the ECU gave. This is the main purpose.
- **Simulate an ECU** ("simulation mode"): implement one method per service and answer a tester, in the same process or on a bus.
- **Read captured messages**: say which service a payload is and what every value means, or read a whole CAN log (`.asc`, `.blf`)
  as diagnostics, like CANoe's trace window.

It is not meant for ECU firmware. KWP2000 and UDS are both supported. The CDD holds no CAN identifiers and no timing, so the
bytes are carried by a transport you provide (section 5).

| Part | What it is |
|---|---|
| `cddgen` | the command line tool (`apps/cddgen`) |
| `cdd-rt` | the small runtime the generated code needs (`crates/cdd-rt`, `no_std` capable) |
| `cdd-codegen` | the generator as a library, for `build.rs` (`crates/cdd-codegen`) |

## 2. Install

You need Rust 1.88 or newer.

```text
git clone https://github.com/najari/cdd_codegen
cd cdd_codegen
cargo build --release -p cddgen          # target/release/cddgen (cddgen.exe on Windows)
cargo install --path apps/cddgen         # or: put it on your PATH
cddgen --help
```

## 3. A quick tour of a CDD

Three commands show what a CDD says before you generate anything. The examples use `testing/synthetic/fixtures/mini-uds.cdd`.

**List the ECUs, variants and services**

```text
$ cddgen inspect mini-uds.cdd
mini-uds.cdd: UDS dtd 13.0.103 encoding utf-8 languages ["en-US", "de-DE"]
ECU Mini variants: Base [base]

Mini/Base: 16 services
DefaultSession_Start    REQ 10 01&7F (2..2)    POS 50 01 (6..6)
TesterPresent_Send      REQ 3E 00&7F (2..2)    POS 7E 00 (2..2)
Battery_Read            REQ 22 01 00 (3..3)    POS 62 01 00 (8..8)
...
```

Each line is the CAPL-style service name, the constant bytes its request and response start with (`&7F` is a mask: the top bit is the
suppress-positive-response bit) and the length range in bytes.

**Show the layout of one service**

```text
$ cddgen inspect mini-uds.cdd --service Battery_Read
POS:
  const SID_PR = 0x62 (8 bits)
  const DataIdentifier = 0x100 (16 bits)
  field Voltage: Voltage_2Byte
  field Temperature: Temperature_1Byte
  field Charging: OffOn_1Byte
  bits Status (8 bits)
    bit 0..+1 Alarm: Flag_1bit
    bit 1..+3 Mode: Mode_3bit
    bit 4..+4 Delta: Signed_4bit
```

**Decode bytes** (`--response` for a response; hex may be written `62 01 00 ...` or `620100...`)

```text
$ cddgen decode mini-uds.cdd --response 62 01 00 31 38 64 01 F3
Battery_Read (Mini/Base/Data/Battery/Read)
    SID_PR          98 (0x62)
    DataIdentifier  256 (0x100)
    Voltage         12600 (0x3138) = 12.6 V
    Temperature     100 (0x64) = 60 degC
    Charging        1 (0x1) 'on'
    Status.Alarm    1 (0x1) 'set'
    Status.Mode     1 (0x1) 'run'
    Status.Delta    -1 (...)
```

**Read a CAN log.** Frames are put together with ISO-TP, matched to the services, and a response is read together with the
request before it. Name the CAN identifiers of tester and ECU with `--id REQUEST:RESPONSE` (hex); without it they are guessed
from the traffic.

```text
$ cddgen decode-log CANSystem.cdd ComfortDiagData.asc --id 700:600
20 frames, 18 diagnostic messages; Any_ECU_example/COMMON_DIAGNOSTICS request 0x700 / response 0x600
    1.051413  0x700  -> REQ  Tester_Present_Send_Response  3E 01
    SID_RQ                                       62 (0x3E)
    SUBFUNCTION_RESPONSE_REQUIRED                1 (0x1)
    1.054365  0x600  <- POS  Tester_Present_Send_Response  7E
    ...
```

Messages the description does not know are shown as `not in the description`; negative responses show the code and its name;
ISO-TP faults (a lost consecutive frame, for example) are reported. `--brief` prints one line per message.

## 4. Generate code

```text
cddgen generate ecu.cdd src/            # writes src/diag.rs and src/manifest.json
```

`OUT_DIR` must exist. Options:

| Option | Meaning |
|---|---|
| `--ecu NAME` | the ECU, when the CDD has several |
| `--variant NAME` | the variant (default: the base variant). `inspect` lists them |
| `--language L` | language of names and labels, e.g. `--language de-DE`; repeat for fallbacks. Default: English, then the CDD's languages |
| `--role tester\|ecu\|both` | `tester`: requests to send and responses to read. `ecu`: simulated-ECU side (`server`). `both` (default) |
| `--strict` | fail if any message has to be left out (otherwise it is skipped and reported) |
| `--default-nrc 0x12` | negative response code a simulated ECU gives services it does not implement |
| `--rt-crate NAME` | name of the runtime crate in the generated code (default `cdd_rt`) |
| `--check` | write nothing; fail if `OUT_DIR` does not already hold exactly this output (use it in CI) |

The command prints how many services were generated and every message left out, with the reason:

```text
29 of 29 services: 29 requests, 28 responses; 1 messages left out
  left out Beispiel_Steuergeraet/.../ReadEnvironmentData POS [CDD-DT-001]: data object `Odometer_Value` has data type COMPTBL, which is not supported yet
```

`diag.rs` starts with a header naming the CDD, ECU, variant and protocol. `manifest.json` records the SHA-256 of the CDD and of
the code, the generator version and the options. **Do not edit `diag.rs`; generate it again.**

## 5. Use the generated code

Add the runtime to your `Cargo.toml` (it is not on crates.io; use a path or git dependency) and make `diag.rs` a module:

```toml
[dependencies]
cdd-rt = { path = "../cdd_codegen/crates/cdd-rt" }
```

```rust
mod diag; // src/diag.rs, written by cddgen
```

The generated code refers to the runtime as `::cdd_rt`. Use `--rt-crate` if you renamed it.

### Test an ECU

Implement `Transport` for whatever carries whole diagnostic messages (ISO-TP over a CAN adapter, DoIP, ...). A message is a
complete payload such as `22 F1 90`, never a CAN frame. Then call services:

```rust
use cdd_rt::tester::{Tester, Transport};
use cdd_rt::Reply;

struct MyBus; // your ISO-TP channel

impl Transport for MyBus {
    type Error = std::io::Error;
    fn send(&mut self, payload: &[u8]) -> Result<(), Self::Error> { /* send one whole message */ }
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> { /* wait for the next, return its length */ }
}

fn read_battery(bus: MyBus) -> Result<(), Box<dyn std::error::Error>> {
    let mut tester = Tester::new(bus);
    let mut rx = [0u8; 4095]; // the response is read out of this buffer

    match tester.call(&diag::service::battery_read::Request, &mut rx)? {
        Reply::Positive(r) => {
            println!("voltage {:?} V", r.voltage.physical());   // Some(12.6)
            println!("charging: {:?}", r.charging.label());      // Some("on")
            println!("mode: {:?}", r.status.mode);               // Run
        }
        Reply::Negative(n) => {
            println!("refused with {:#04x} {:?}", n.nrc, diag::nrc::name(n.nrc));
        }
    }
    Ok(())
}
```

- `Tester::call` sends the request and returns the `Reply`. It waits out "response pending" (`0x78`) up to `tester.max_pending`
  times (default 16), then fails with `CallError::TooManyPending`. Time-outs belong to your `Transport`.
- Use `tester.send(&request)` for requests that get no answer (for example a request with the suppress bit set).
- In a test, `reply.expect_positive()` returns the response or panics; `reply.expect_nrc(0x31)` asserts a given negative response.
  `reply.response_code()` gives the code of a negative response as an `Option<u8>`.
- Requests are plain structs; you fill the fields, and what you do not set (service id, constants) is written for you:

```rust
tester.call(&diag::service::vin_write::Request { vin: *b"WVWZZZ1JZXW000001" }, &mut rx)?.expect_positive();
tester.send(&diag::service::tester_present_send::Request { suppress_positive_response: true })?; // 3E 80
```

### Simulate an ECU

Implement `Handler`: one `on_<service>` method per service you want to answer. Services you leave out are refused with
`Handler::DEFAULT_NRC` (`0x12`, or the value of `--default-nrc`). `Loopback` connects a tester and the simulated ECU in one process;
to serve a real bus, give `Server` the requests your transport receives (`Server` implements `cdd_rt::ecu::Responder`).

```rust
use cdd_rt::sim::Loopback;
use cdd_rt::tester::Tester;
use cdd_rt::Reply;
use diag::server::{Handler, Server};
use diag::service::{battery_read, vin_write};

#[derive(Default)]
struct MyEcu { vin: [u8; 17] }

impl Handler for MyEcu {
    fn on_battery_read(&mut self, _request: &battery_read::Request) -> Reply<battery_read::Response> {
        Reply::Positive(battery_read::Response {
            voltage: diag::types::Voltage2byte::from_physical(12.6).unwrap(),
            temperature: diag::types::Temperature1byte::from_physical(21.0).unwrap(),
            charging: diag::types::OffOn1byte::from_raw(0),
            status: battery_read::Status {
                alarm: diag::types::Flag1bit::from_raw(0),
                mode: diag::types::Mode3bit::Run,
                delta: 0,
            },
        })
    }

    fn on_vin_write(&mut self, request: &vin_write::Request) -> Reply<vin_write::Response> {
        if request.vin.iter().any(|b| !b.is_ascii_alphanumeric()) {
            return Reply::nrc(0x2E, diag::nrc::REQUEST_OUT_OF_RANGE); // (service id, code)
        }
        self.vin = request.vin;
        Reply::Positive(vin_write::Response)
    }
}

let mut tester = Tester::new(Loopback::new(Server::new(MyEcu::default())));
```

`Loopback` keeps every message in `tester.transport.traffic` as `(Direction, bytes)` so a test can check the exact bytes.
A response may borrow from the handler (`Response<'_>`), so lists and byte fields can point at the ECU's own state without copying.
The server honors the suppress-positive-response bit: a positive answer to such a request is not sent; a negative one is.

### Read captured messages

```rust
let payload = [0x62, 0x01, 0x00, 0x31, 0x38, 0x64, 0x01, 0xF3];
let message = diag::AnyResponse::decode(&payload)   // None: no service of the description
    .expect("a response of the description")        // Some(Err(_)): right service, wrong layout
    .expect("well formed");
println!("{}", message.name());                      // Battery_Read
message.describe(&mut my_visitor);                   // every value: path, raw, name, physical value, unit
```

`AnyRequest` does the same for requests. `describe` calls your `cdd_rt::Visitor` once per value, with its path
(`Status.Mode`, `ListOfDtc[2].DTC`), raw value, symbolic text, physical value and unit. It allocates nothing.

### Generate from `build.rs`

Instead of committing `diag.rs`, add `cdd-codegen` as a build dependency:

```rust
use cdd_codegen::{Config, Role};

let bytes = std::fs::read("diag/ecu.cdd").unwrap();
let config = Config { role: Role::Tester, variant: Some("Base".into()), ..Config::default() };
let artifacts = config.generate("ecu.cdd", &bytes).unwrap();
for s in &artifacts.report.skipped {
    println!("cargo:warning=left out {} {}: {}", s.service, s.message, s.reason);
}
std::fs::write(out_dir.join("diag.rs"), artifacts.code).unwrap();
// then: mod diag { include!(concat!(env!("OUT_DIR"), "/diag.rs")); }
```

## 6. Reference

### What is generated

| Item | Contents |
|---|---|
| `service::<name>::Request` / `Response` | one struct per message, with fields for everything that is not a constant. They implement `Decode`, `Encode`, `Message`, `Describe`; a request also implements `Request` (usable with `Tester::call`) |
| `types` | enums and number types for the data types of the CDD |
| `AnyRequest`, `AnyResponse` | one variant per service; `decode`, `name`, `describe`, `encode` |
| `server::{Handler, Server}` | simulated ECU (roles `ecu` and `both`) |
| `nrc` | negative response code constants (`REQUEST_OUT_OF_RANGE`), `nrc::name(code)` |
| `SERVICES` | table of every service: name, key, service id, whether it has a request and a response, its negative response codes, functional/physical addressing |
| `DOCUMENT`, `SOURCE_SHA256`, `ECU`, `VARIANT`, `PROTOCOL` | where the code came from |

### Names

A service is named like in CAPL: the short-cut name of the CDD, or `<DiagInst>_<Service>` (`Battery_Read`). The module is its
`snake_case` form (`battery_read`) and the `AnyRequest` variant its `CamelCase` form (`BatteryRead`). Fields are `snake_case`;
a repeated name gets a numeric suffix. Names and labels come from the language you choose with `--language`.

### Types

| In the CDD | In Rust |
|---|---|
| unsigned / signed integer, 1 to 8 bytes or fewer bits | `u8`..`u64` / `i8`..`i64`; big or little endian as the CDD says |
| BCD number | unsigned integer holding the decimal value (`0x12 0x34` is `1234`) |
| float / double | `f32` / `f64` |
| fixed-length ASCII or bytes | `[u8; N]` |
| variable-length bytes | `&[u8]` (so the message has a lifetime) |
| text table | `enum` when every entry is one value, otherwise a number type with `label()`; an unnamed value is `Other(raw)` |
| linear conversion | number type with `raw()`, `from_raw()`, `physical()`, `from_physical()` and `UNIT` |
| structure | `struct` of its members |
| bit container | `struct` of its members; the first member takes the least significant bits |
| multiplexer | `enum` with one variant per case (`Case11`) and `Default` for a selector no case names |
| repeated part | `List<'a, Element>`: `len()`, `iter()` on a decoded message; `List::Items(&[...])` to build one |
| suppress-positive-response bit | `suppress_positive_response: bool` in the request |

### Command line

```text
cddgen generate   <cdd> <out_dir> [options]            see section 4
cddgen inspect    <cdd> [--service NAME] [--ecu E] [--variant V] [--language L]
cddgen decode     <cdd> HEX... [--response] [--ecu E] [--variant V] [--language L]
cddgen decode-log <cdd> <log> [--id REQ:RESP]... [--channel N] [--ecu E] [--variant V] [--language L] [--brief]
```

Every command also takes `--help`. `decode-log` reads `.asc` and `.blf`.

## 7. Good to know

- **The CDD has no CAN identifiers or timing.** Give `decode-log` the identifiers with `--id`; give the generated code a
  `Transport` that does ISO-TP, addressing and time-outs.
- **Which service is a message?** `AnyRequest::decode` compares the constant bytes (service id, sub-function, identifier) and
  prefers the service that fixes the most bytes. Two services with the same constants cannot be told apart; `generate` lists such
  pairs as `ambiguous`.
- **Wrong replies:** a payload that begins like a service but does not fit its layout is `Some(Err(_))`. A simulated ECU answers it
  with `0x13` (incorrect length or invalid format). A payload that fits no service goes to `Handler::on_unknown`, which answers `0x11`
  (service not supported) unless you override it (return `None` to stay silent).
- **Strict and lenient.** Generated code is strict: a BCD number with a nibble above 9 or a count that disagrees with the list is
  an error. `inspect`, `decode` and `decode-log` are lenient on purpose (raw bytes and a note), so a bad log can still be read.
- **A count field is a normal field.** A response with "number of DTCs" followed by that many entries needs both set and equal;
  to send a deliberately wrong count, send raw bytes through your transport.
- **Bit order.** The first member of a bit container takes the least significant bits.
- **Reproducible output.** The same CDD and options give the same bytes, so `--check` works in CI.

## 8. Troubleshooting

| Message or symptom | What to do |
|---|---|
| `no CAN identifier of the log carries requests of this description` | name the identifiers: `--id 700:600`; check `--ecu` / `--variant` |
| `not in the description` in `decode-log` | the message is not a service of this ECU / variant (try another `--variant`), or `--id` pairs the wrong identifiers |
| `left out ... [CDD-DT-001]` | the message uses a data type that is not supported yet (section 9); other services are unaffected. `--strict` makes it an error |
| `no variant ... in ECU ...` / `no ECU ...` | run `cddgen inspect <cdd>` to see the ECUs and variants |
| `does not hold exactly this output` (`--check`) | `diag.rs` is older than the CDD or was edited: run `generate` again |
| `ISO-TP fault` in `decode-log` | frames were lost or interleaved in the log; the line says what was expected |
| A name looks wrong | choose another language with `--language`; names follow the CDD's short-cut names |

## 9. Limitations

- Not supported: data types `VALTBL`, `COMPTBL`, `FORMULADT`; repeated parts whose elements vary in size (`ListOfParameters`,
  extended data records); `DOMAINDATAPROXYCOMP` (UDS 5.0.4). Messages that use them are left out and reported.
- Logs: CAN FD frames in `.asc` are not read; `.blf` objects of types 1, 86, 100 and 101 only; ISO-TP with normal addressing.
- The generator has been checked against CANoe's sample CDD files and logs and against small CDD files written for this project.
  It was also **spot-checked against CANdelaStudio 13** on `SampleUDS.cdd`: the byte layout of the `ReadDtcInformation` services
  (a counted-to-the-end list of 3-byte DTC plus status byte), the bit positions and reserved bits of the DTC status byte, and the
  multiplexer of the extended data record (`0x01` Occurrence Counter, `0x02` Healing Counter, `0x03` Condition as 2 bytes, any
  other selector value uses the default structure, which is what the top `STRUCTURE` of a `MUXDT` is) all agree. This is not an
  exhaustive comparison, and CANoe's own interpretation of messages (its trace and diagnostic console need a full license)
  has not been compared. The header of `diag.rs` says so (**Experimental**).

## 10. Verification and development

```text
cargo test --workspace                       # all tests; the CANoe-sample tests are skipped if the samples are missing
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

The CANoe sample tests look for `Sample Configurations .../CAN` in the default Vector install location; set `CANOE_SAMPLES` to
another folder.

| Path | Role |
|---|---|
| `crates/cdd-model` | CDD parser, resolution of the three layers (instance, template, protocol), layout IR, **interpreter** |
| `crates/cdd-codegen` | IR to Rust (`quote`, `syn`, `prettyplease`) |
| `crates/cdd-rt` | runtime: `Reader`/`Writer`, `Tester`, `Responder`, `Loopback` |
| `crates/cdd-log` | `.asc` / `.blf` readers, ISO-TP reassembly |
| `apps/cddgen` | command line |
| `testing/harness` | differential checker: generated code against the interpreter |
| `testing/synthetic` | small CDD files (`fixtures/make_fixtures.py` writes them), hand-computed wire tests, simulation tests, the examples of this manual (`tests/manual.rs`) |
| `testing/canoe-samples` | CANoe samples: differential test, real log decoding, simulation |
| `docs/` | design review of CDD to Rust generation, early prototype |

How it is checked: the interpreter and the generator read the same layout, so for every service random values are encoded by
the interpreter and the generated code must read them identically, write the same bytes, and give the same verdict on truncated,
extended, bit-flipped and random payloads. Because both read the same layout, the layout itself is checked with hand-computed
bytes and with real CAN logs.

## 11. Status and license

Version 0.1.0, profile maturity **Experimental**. The license has not been decided yet (`publish = false`); until a license file
is added, no permission to reuse the code is granted.

---

# 한국어

[English](#english) · **한국어**

## 목차

1. [소개](#1-소개)
2. [설치](#2-설치)
3. [CDD 둘러보기](#3-cdd-둘러보기)
4. [코드 생성](#4-코드-생성)
5. [생성된 코드 사용하기](#5-생성된-코드-사용하기)
6. [참조](#6-참조)
7. [알아 둘 것](#7-알아-둘-것)
8. [문제 해결](#8-문제-해결)
9. [한계](#9-한계)
10. [검증과 개발](#10-검증과-개발)
11. [상태와 라이선스](#11-상태와-라이선스)

## 1. 소개

`cddgen`은 **CDD** 파일(CANdela / CANdelaStudio가 쓰는 진단 기술 파일)을 읽어, 그 안의 UDS 또는 KWP2000 서비스에 대한
**Rust 코드**를 만듭니다. 생성된 코드로 할 수 있는 일은 다음과 같습니다.

- **ECU 테스트**: CANoe처럼 요청을 보내고, 타입이 있는 응답을 받아 값·이름·단위를 확인하고, ECU가 돌려준 부정 응답 코드를
  봅니다. 이것이 주된 용도입니다.
- **ECU 시뮬레이션** ("simulation mode"): 서비스마다 메서드 하나를 구현해 테스터에게 응답합니다. 같은 프로세스 안에서도,
  버스 위에서도 쓸 수 있습니다.
- **캡처한 메시지 읽기**: 페이로드가 어떤 서비스이고 값이 무엇을 뜻하는지 알려 주거나, CAN 로그(`.asc`, `.blf`) 전체를
  CANoe의 trace 창처럼 진단으로 해석합니다.

ECU 펌웨어에 넣는 용도는 아닙니다. KWP2000과 UDS를 모두 지원합니다. CDD에는 CAN 식별자와 타이밍이 없으므로, 바이트를 실어
나르는 일은 직접 만드는 transport가 합니다(5절).

| 구성 | 설명 |
|---|---|
| `cddgen` | 명령줄 도구 (`apps/cddgen`) |
| `cdd-rt` | 생성된 코드가 쓰는 작은 런타임 (`crates/cdd-rt`, `no_std` 가능) |
| `cdd-codegen` | `build.rs`에서 쓸 수 있는 라이브러리 형태의 생성기 (`crates/cdd-codegen`) |

## 2. 설치

Rust 1.88 이상이 필요합니다.

```text
git clone https://github.com/najari/cdd_codegen
cd cdd_codegen
cargo build --release -p cddgen          # target/release/cddgen (Windows는 cddgen.exe)
cargo install --path apps/cddgen         # 또는 PATH에 넣기
cddgen --help
```

## 3. CDD 둘러보기

코드를 만들기 전에 CDD가 무엇을 말하는지 세 명령으로 볼 수 있습니다. 예제는 `testing/synthetic/fixtures/mini-uds.cdd`를 씁니다.

**ECU, variant, 서비스 목록**

```text
$ cddgen inspect mini-uds.cdd
mini-uds.cdd: UDS dtd 13.0.103 encoding utf-8 languages ["en-US", "de-DE"]
ECU Mini variants: Base [base]

Mini/Base: 16 services
DefaultSession_Start    REQ 10 01&7F (2..2)    POS 50 01 (6..6)
TesterPresent_Send      REQ 3E 00&7F (2..2)    POS 7E 00 (2..2)
Battery_Read            REQ 22 01 00 (3..3)    POS 62 01 00 (8..8)
...
```

한 줄은 CAPL 방식의 서비스 이름, 요청과 응답이 시작하는 고정 바이트(`&7F`는 마스크로, 최상위 비트가 suppress-positive-response
비트라는 뜻), 바이트 길이 범위입니다.

**서비스 하나의 레이아웃**

```text
$ cddgen inspect mini-uds.cdd --service Battery_Read
POS:
  const SID_PR = 0x62 (8 bits)
  const DataIdentifier = 0x100 (16 bits)
  field Voltage: Voltage_2Byte
  field Temperature: Temperature_1Byte
  field Charging: OffOn_1Byte
  bits Status (8 bits)
    bit 0..+1 Alarm: Flag_1bit
    bit 1..+3 Mode: Mode_3bit
    bit 4..+4 Delta: Signed_4bit
```

**바이트 해석** (응답이면 `--response`; 16진수는 `62 01 00 ...` 또는 `620100...`으로 씁니다)

```text
$ cddgen decode mini-uds.cdd --response 62 01 00 31 38 64 01 F3
Battery_Read (Mini/Base/Data/Battery/Read)
    SID_PR          98 (0x62)
    DataIdentifier  256 (0x100)
    Voltage         12600 (0x3138) = 12.6 V
    Temperature     100 (0x64) = 60 degC
    Charging        1 (0x1) 'on'
    Status.Alarm    1 (0x1) 'set'
    Status.Mode     1 (0x1) 'run'
    Status.Delta    -1 (...)
```

**CAN 로그 읽기.** 프레임을 ISO-TP로 다시 조립해 서비스와 맞추고, 응답은 바로 앞의 요청과 함께 해석합니다. 테스터와 ECU의
CAN 식별자는 `--id 요청:응답`(16진수)으로 지정합니다. 생략하면 트래픽에서 추정합니다.

```text
$ cddgen decode-log CANSystem.cdd ComfortDiagData.asc --id 700:600
20 frames, 18 diagnostic messages; Any_ECU_example/COMMON_DIAGNOSTICS request 0x700 / response 0x600
    1.051413  0x700  -> REQ  Tester_Present_Send_Response  3E 01
    SID_RQ                                       62 (0x3E)
    SUBFUNCTION_RESPONSE_REQUIRED                1 (0x1)
    1.054365  0x600  <- POS  Tester_Present_Send_Response  7E
    ...
```

CDD에 없는 메시지는 `not in the description`으로 표시하고, 부정 응답은 코드와 이름을 보여 주며, ISO-TP 오류(예: 빠진
consecutive frame)는 따로 알려 줍니다. `--brief`는 메시지당 한 줄만 출력합니다.

## 4. 코드 생성

```text
cddgen generate ecu.cdd src/            # src/diag.rs 와 src/manifest.json 을 씁니다
```

`OUT_DIR`은 이미 있는 디렉터리여야 합니다. 옵션:

| 옵션 | 의미 |
|---|---|
| `--ecu NAME` | CDD에 ECU가 여럿일 때 선택 |
| `--variant NAME` | variant (기본: base variant). `inspect`로 목록을 볼 수 있음 |
| `--language L` | 이름과 레이블의 언어. 예: `--language de-DE`. 대체 언어는 반복해서 지정. 기본: 영어, 그다음 CDD의 언어 |
| `--role tester\|ecu\|both` | `tester`: 보낼 요청과 읽을 응답. `ecu`: 시뮬레이션 ECU 쪽(`server`). `both`(기본값) |
| `--strict` | 건너뛰어야 하는 메시지가 하나라도 있으면 실패 (기본은 건너뛰고 보고) |
| `--default-nrc 0x12` | 시뮬레이션 ECU가 구현하지 않은 서비스에 주는 부정 응답 코드 |
| `--rt-crate NAME` | 생성 코드에서 런타임 crate를 부르는 이름 (기본 `cdd_rt`) |
| `--check` | 쓰지 않고, `OUT_DIR`이 이 출력과 정확히 같지 않으면 실패 (CI용) |

명령은 생성된 서비스 수와, 건너뛴 메시지 및 그 이유를 출력합니다.

```text
29 of 29 services: 29 requests, 28 responses; 1 messages left out
  left out Beispiel_Steuergeraet/.../ReadEnvironmentData POS [CDD-DT-001]: data object `Odometer_Value` has data type COMPTBL, which is not supported yet
```

`diag.rs`의 머리말에는 CDD, ECU, variant, 프로토콜이 적혀 있습니다. `manifest.json`에는 CDD와 코드의 SHA-256, 생성기
버전, 옵션이 기록됩니다. **`diag.rs`는 고치지 말고 다시 생성하세요.**

## 5. 생성된 코드 사용하기

`Cargo.toml`에 런타임을 추가하고(crates.io에는 없으므로 path 또는 git 의존성), `diag.rs`를 모듈로 선언합니다.

```toml
[dependencies]
cdd-rt = { path = "../cdd_codegen/crates/cdd-rt" }
```

```rust
mod diag; // cddgen이 쓴 src/diag.rs
```

생성된 코드는 런타임을 `::cdd_rt`로 부릅니다. 이름을 바꿨다면 `--rt-crate`를 쓰세요.

### ECU 테스트하기

진단 메시지 한 건을 통째로 실어 나르는 것(CAN 어댑터 위의 ISO-TP, DoIP 등)에 `Transport`를 구현합니다. 메시지는 `22 F1 90`
같은 완성된 페이로드이며 CAN 프레임이 아닙니다. 그다음 서비스를 호출합니다.

```rust
use cdd_rt::tester::{Tester, Transport};
use cdd_rt::Reply;

struct MyBus; // 직접 만든 ISO-TP 채널

impl Transport for MyBus {
    type Error = std::io::Error;
    fn send(&mut self, payload: &[u8]) -> Result<(), Self::Error> { /* 메시지 한 건 전송 */ }
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> { /* 다음 메시지를 기다려 길이를 반환 */ }
}

fn read_battery(bus: MyBus) -> Result<(), Box<dyn std::error::Error>> {
    let mut tester = Tester::new(bus);
    let mut rx = [0u8; 4095]; // 응답은 이 버퍼에서 읽습니다

    match tester.call(&diag::service::battery_read::Request, &mut rx)? {
        Reply::Positive(r) => {
            println!("voltage {:?} V", r.voltage.physical());   // Some(12.6)
            println!("charging: {:?}", r.charging.label());      // Some("on")
            println!("mode: {:?}", r.status.mode);               // Run
        }
        Reply::Negative(n) => {
            println!("refused with {:#04x} {:?}", n.nrc, diag::nrc::name(n.nrc));
        }
    }
    Ok(())
}
```

- `Tester::call`은 요청을 보내고 `Reply`를 돌려줍니다. "response pending"(`0x78`)은 `tester.max_pending`번(기본 16)까지
  기다리고, 넘으면 `CallError::TooManyPending`으로 실패합니다. 타임아웃은 `Transport`가 맡습니다.
- 응답이 없는 요청(예: suppress 비트를 켠 요청)은 `tester.send(&request)`로 보냅니다.
- 테스트에서는 `reply.expect_positive()`가 응답을 꺼내거나 panic하고, `reply.expect_nrc(0x31)`은 특정 부정 응답을 확인합니다.
  `reply.response_code()`는 부정 응답의 코드를 `Option<u8>`로 줍니다.
- 요청은 평범한 구조체입니다. 필드만 채우면 서비스 ID 같은 상수는 알아서 써 줍니다.

```rust
tester.call(&diag::service::vin_write::Request { vin: *b"WVWZZZ1JZXW000001" }, &mut rx)?.expect_positive();
tester.send(&diag::service::tester_present_send::Request { suppress_positive_response: true })?; // 3E 80
```

### ECU 시뮬레이션하기

`Handler`를 구현합니다. 응답하려는 서비스마다 `on_<서비스>` 메서드 하나입니다. 구현하지 않은 서비스는
`Handler::DEFAULT_NRC`(`0x12`, 또는 `--default-nrc` 값)로 거절합니다. `Loopback`은 테스터와 시뮬레이션 ECU를 한 프로세스에
연결합니다. 실제 버스에서 서비스하려면 transport가 받은 요청을 `Server`에 넘기세요(`Server`는 `cdd_rt::ecu::Responder`를
구현합니다).

```rust
use cdd_rt::sim::Loopback;
use cdd_rt::tester::Tester;
use cdd_rt::Reply;
use diag::server::{Handler, Server};
use diag::service::{battery_read, vin_write};

#[derive(Default)]
struct MyEcu { vin: [u8; 17] }

impl Handler for MyEcu {
    fn on_battery_read(&mut self, _request: &battery_read::Request) -> Reply<battery_read::Response> {
        Reply::Positive(battery_read::Response {
            voltage: diag::types::Voltage2byte::from_physical(12.6).unwrap(),
            temperature: diag::types::Temperature1byte::from_physical(21.0).unwrap(),
            charging: diag::types::OffOn1byte::from_raw(0),
            status: battery_read::Status {
                alarm: diag::types::Flag1bit::from_raw(0),
                mode: diag::types::Mode3bit::Run,
                delta: 0,
            },
        })
    }

    fn on_vin_write(&mut self, request: &vin_write::Request) -> Reply<vin_write::Response> {
        if request.vin.iter().any(|b| !b.is_ascii_alphanumeric()) {
            return Reply::nrc(0x2E, diag::nrc::REQUEST_OUT_OF_RANGE); // (서비스 ID, 코드)
        }
        self.vin = request.vin;
        Reply::Positive(vin_write::Response)
    }
}

let mut tester = Tester::new(Loopback::new(Server::new(MyEcu::default())));
```

`Loopback`은 오간 메시지를 `tester.transport.traffic`에 `(Direction, 바이트)`로 남기므로 테스트에서 정확한 바이트를 확인할
수 있습니다. 응답은 핸들러를 빌릴 수 있어서(`Response<'_>`), 목록이나 바이트 필드가 복사 없이 ECU의 상태를 가리킬 수 있습니다.
서버는 suppress-positive-response 비트를 지킵니다. 이 비트가 켜진 요청의 긍정 응답은 보내지 않고, 부정 응답은 보냅니다.

### 캡처한 메시지 읽기

```rust
let payload = [0x62, 0x01, 0x00, 0x31, 0x38, 0x64, 0x01, 0xF3];
let message = diag::AnyResponse::decode(&payload)   // None: CDD의 어떤 서비스도 아님
    .expect("a response of the description")        // Some(Err(_)): 서비스는 맞지만 레이아웃이 틀림
    .expect("well formed");
println!("{}", message.name());                      // Battery_Read
message.describe(&mut my_visitor);                   // 모든 값: 경로, 원시값, 이름, 물리값, 단위
```

요청은 `AnyRequest`로 같은 일을 합니다. `describe`는 값마다 `cdd_rt::Visitor`를 한 번씩 호출하며, 경로(`Status.Mode`,
`ListOfDtc[2].DTC`), 원시값, 기호 텍스트, 물리값, 단위를 넘깁니다. 메모리를 할당하지 않습니다.

### `build.rs`에서 생성하기

`diag.rs`를 커밋하는 대신 `cdd-codegen`을 빌드 의존성으로 추가할 수 있습니다.

```rust
use cdd_codegen::{Config, Role};

let bytes = std::fs::read("diag/ecu.cdd").unwrap();
let config = Config { role: Role::Tester, variant: Some("Base".into()), ..Config::default() };
let artifacts = config.generate("ecu.cdd", &bytes).unwrap();
for s in &artifacts.report.skipped {
    println!("cargo:warning=left out {} {}: {}", s.service, s.message, s.reason);
}
std::fs::write(out_dir.join("diag.rs"), artifacts.code).unwrap();
// 그다음: mod diag { include!(concat!(env!("OUT_DIR"), "/diag.rs")); }
```

## 6. 참조

### 생성되는 것

| 항목 | 내용 |
|---|---|
| `service::<이름>::Request` / `Response` | 메시지마다 구조체 하나. 상수가 아닌 모든 것이 필드. `Decode`, `Encode`, `Message`, `Describe`를 구현하고, 요청은 `Request`도 구현(`Tester::call`에 사용) |
| `types` | CDD의 데이터 타입에 해당하는 열거형과 숫자 타입 |
| `AnyRequest`, `AnyResponse` | 서비스마다 variant 하나. `decode`, `name`, `describe`, `encode` |
| `server::{Handler, Server}` | 시뮬레이션 ECU (역할 `ecu`, `both`) |
| `nrc` | 부정 응답 코드 상수(`REQUEST_OUT_OF_RANGE`), `nrc::name(code)` |
| `SERVICES` | 모든 서비스의 표: 이름, 키, 서비스 ID, 요청·응답 유무, 부정 응답 코드, functional/physical 주소 지정 |
| `DOCUMENT`, `SOURCE_SHA256`, `ECU`, `VARIANT`, `PROTOCOL` | 코드의 출처 |

### 이름

서비스 이름은 CAPL과 같습니다. CDD의 short-cut 이름, 없으면 `<DiagInst>_<Service>`(`Battery_Read`)입니다. 모듈은 `snake_case`
(`battery_read`), `AnyRequest`의 variant는 `CamelCase`(`BatteryRead`)입니다. 필드는 `snake_case`이고, 같은 이름이 겹치면 숫자
접미사가 붙습니다. 이름과 레이블은 `--language`로 고른 언어를 따릅니다.

### 타입

| CDD | Rust |
|---|---|
| 부호 없는 / 부호 있는 정수 (1~8바이트 또는 그보다 적은 비트) | `u8`..`u64` / `i8`..`i64`. CDD가 정한 big / little endian |
| BCD 수 | 십진 값을 담은 부호 없는 정수 (`0x12 0x34`는 `1234`) |
| float / double | `f32` / `f64` |
| 길이가 고정된 ASCII 또는 바이트 | `[u8; N]` |
| 길이가 가변인 바이트 | `&[u8]` (그래서 메시지에 lifetime이 붙음) |
| 텍스트 테이블 | 모든 항목이 값 하나면 `enum`, 아니면 `label()`이 있는 숫자 타입. 이름 없는 값은 `Other(raw)` |
| 선형 변환 | `raw()`, `from_raw()`, `physical()`, `from_physical()`, `UNIT`이 있는 숫자 타입 |
| 구조체 | 멤버로 이루어진 `struct` |
| 비트 컨테이너 | 멤버로 이루어진 `struct`. 첫 멤버가 최하위 비트 |
| 멀티플렉서 | case마다 variant 하나(`Case11`), 어느 case도 아닌 선택값은 `Default` |
| 반복 구조 | `List<'a, Element>`: 디코딩한 메시지는 `len()`, `iter()`, 만들 때는 `List::Items(&[...])` |
| suppress-positive-response 비트 | 요청의 `suppress_positive_response: bool` |

### 명령줄

```text
cddgen generate   <cdd> <out_dir> [옵션]                    4절 참조
cddgen inspect    <cdd> [--service NAME] [--ecu E] [--variant V] [--language L]
cddgen decode     <cdd> HEX... [--response] [--ecu E] [--variant V] [--language L]
cddgen decode-log <cdd> <log> [--id REQ:RESP]... [--channel N] [--ecu E] [--variant V] [--language L] [--brief]
```

모든 명령에서 `--help`를 쓸 수 있습니다. `decode-log`는 `.asc`와 `.blf`를 읽습니다.

## 7. 알아 둘 것

- **CDD에는 CAN 식별자와 타이밍이 없습니다.** `decode-log`에는 `--id`로 식별자를 주고, 생성된 코드에는 ISO-TP, 주소 지정,
  타임아웃을 처리하는 `Transport`를 붙이세요.
- **메시지가 어느 서비스인지**는 `AnyRequest::decode`가 고정 바이트(서비스 ID, sub-function, 식별자)를 비교해 정하며, 고정
  바이트가 가장 많은 서비스를 우선합니다. 고정 바이트가 같은 서비스끼리는 구분할 수 없고, `generate`가 이를 `ambiguous`로
  알려 줍니다.
- **맞지 않는 메시지:** 어떤 서비스처럼 시작하지만 레이아웃이 맞지 않으면 `Some(Err(_))`이고, 시뮬레이션 ECU는 `0x13`(길이
  또는 형식 오류)으로 답합니다. 어느 서비스에도 맞지 않으면 `Handler::on_unknown`으로 가며, 기본은 `0x11`(서비스 미지원)입니다.
  재정의해서 `None`을 반환하면 응답하지 않습니다.
- **생성 코드는 엄격하고, 도구는 관대합니다.** 생성된 코드에서는 9를 넘는 BCD 니블이나 목록과 맞지 않는 개수가 오류입니다.
  `inspect`, `decode`, `decode-log`는 잘못된 로그도 읽을 수 있도록 일부러 관대하게 원시 바이트와 주석으로 보여 줍니다.
- **개수 필드도 보통 필드입니다.** "DTC 개수" 뒤에 그만큼의 항목이 오는 응답은 둘을 같게 채워야 합니다. 일부러 틀린 개수를
  보내려면 transport로 원시 바이트를 보내세요.
- **비트 순서:** 비트 컨테이너의 첫 멤버가 최하위 비트를 씁니다.
- **재현 가능한 출력:** 같은 CDD와 옵션이면 출력 바이트가 같으므로 CI에서 `--check`를 쓸 수 있습니다.

## 8. 문제 해결

| 메시지 또는 증상 | 할 일 |
|---|---|
| `no CAN identifier of the log carries requests of this description` | `--id 700:600`처럼 식별자를 지정하고, `--ecu` / `--variant`를 확인 |
| `decode-log`에서 `not in the description` | 이 ECU / variant의 서비스가 아니거나(다른 `--variant`를 시도) `--id`가 엉뚱한 식별자를 짝지음 |
| `left out ... [CDD-DT-001]` | 아직 지원하지 않는 데이터 타입을 쓰는 메시지 (9절). 다른 서비스에는 영향 없음. `--strict`면 오류 |
| `no variant ... in ECU ...` / `no ECU ...` | `cddgen inspect <cdd>`로 ECU와 variant 확인 |
| `does not hold exactly this output` (`--check`) | `diag.rs`가 CDD보다 오래되었거나 손으로 고쳐짐: `generate`를 다시 실행 |
| `decode-log`의 `ISO-TP fault` | 로그에서 프레임이 빠졌거나 섞임. 줄에 기대한 값이 적혀 있음 |
| 이름이 이상함 | `--language`로 다른 언어를 고르세요. 이름은 CDD의 short-cut 이름을 따릅니다 |

## 9. 한계

- 지원하지 않음: 데이터 타입 `VALTBL`, `COMPTBL`, `FORMULADT`, 원소 크기가 달라지는 반복 구조(`ListOfParameters`, 확장 데이터
  레코드), `DOMAINDATAPROXYCOMP`(UDS 5.0.4). 이를 쓰는 메시지는 건너뛰고 보고합니다.
- 로그: `.asc`의 CAN FD 프레임은 읽지 않고, `.blf`는 객체 타입 1, 86, 100, 101만 읽으며, ISO-TP는 normal addressing만 지원합니다.
- 생성기는 CANoe 샘플 CDD·로그와 이 프로젝트를 위해 만든 작은 CDD로 검증했습니다. 또한 `SampleUDS.cdd`로 **CANdelaStudio 13과
  일부 대조**했습니다. `ReadDtcInformation` 서비스의 바이트 구성(끝까지 반복되는 3바이트 DTC + 상태 바이트), DTC 상태 바이트의
  비트 위치와 reserved 비트, 확장 데이터 레코드의 멀티플렉서(`0x01` Occurrence Counter, `0x02` Healing Counter, `0x03`
  Condition 2바이트, 그 밖의 선택값은 기본 구조 — `MUXDT` 맨 위 `STRUCTURE`가 그것)가 모두 일치했습니다. 전수 비교는 아니며,
  CANoe가 메시지를 해석한 결과(Trace, Diagnostic Console은 정식 라이선스가 필요)는 비교하지 못했습니다.
  `diag.rs` 머리말에도 그렇게 적혀 있습니다(**Experimental**).

## 10. 검증과 개발

```text
cargo test --workspace                       # 전체 테스트. CANoe 샘플이 없으면 해당 테스트는 건너뜀
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

CANoe 샘플 테스트는 Vector 기본 설치 위치의 `Sample Configurations .../CAN`을 찾습니다. 다른 곳이면 `CANOE_SAMPLES`
환경 변수로 지정하세요.

| 경로 | 역할 |
|---|---|
| `crates/cdd-model` | CDD 파서, 세 계층(인스턴스·템플릿·프로토콜) 해석, 레이아웃 IR, **인터프리터** |
| `crates/cdd-codegen` | IR에서 Rust로 (`quote`, `syn`, `prettyplease`) |
| `crates/cdd-rt` | 런타임: `Reader`/`Writer`, `Tester`, `Responder`, `Loopback` |
| `crates/cdd-log` | `.asc` / `.blf` 읽기, ISO-TP 재조립 |
| `apps/cddgen` | 명령줄 |
| `testing/harness` | 차분 검증 도구: 생성 코드와 인터프리터 비교 |
| `testing/synthetic` | 작은 CDD(`fixtures/make_fixtures.py`가 생성), 손으로 계산한 바이트 테스트, 시뮬레이션 테스트, 이 설명서의 예제(`tests/manual.rs`) |
| `testing/canoe-samples` | CANoe 샘플: 차분 테스트, 실제 로그 해석, 시뮬레이션 |
| `docs/` | CDD→Rust 생성 설계 검토 문서와 초기 프로토타입 |

검증 방식: 인터프리터와 생성기가 같은 레이아웃을 읽으므로, 서비스마다 인터프리터가 무작위 값을 인코딩하고 생성 코드가 이를
똑같이 읽고 같은 바이트로 다시 쓰는지 봅니다. 잘리거나 늘어나거나 비트가 뒤집힌 페이로드, 무작위 페이로드에도 같은 판정을
내려야 합니다. 둘이 같은 레이아웃을 읽기 때문에, 레이아웃 자체는 손으로 계산한 바이트와 실제 CAN 로그로 확인합니다.

## 11. 상태와 라이선스

버전 0.1.0, 프로파일 성숙도 **Experimental**. 라이선스는 아직 정하지 않았습니다(`publish = false`). 라이선스 파일이
추가되기 전까지는 코드를 재사용할 권한이 부여되지 않은 상태입니다.
