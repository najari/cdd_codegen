# cdd_code_gen

CANdela **CDD** 파일에서 **Rust 진단 코드**를 만드는 코드 생성기입니다. UDS와 KWP2000을 다룹니다.

주 사용자는 **테스터**입니다. CANoe처럼 진단 요청을 보내고 응답을 해석하는 테스트 코드를 만드는 것이 첫 번째 목적이고,
같은 CDD로 **시뮬레이션 ECU**(CANoe의 simulation mode)도 만듭니다. 실제 ECU 펌웨어에 들어가는 코드는 목표가 아닙니다.
(런타임은 `no_std`로 빌드되지만, 그것은 ECU 펌웨어용이라는 보증이 아니라 의존성을 가볍게 하려는 것입니다.)

> 라이선스는 아직 정하지 않았습니다. `publish = false`입니다.
> 프로파일 성숙도는 **Experimental**입니다. CANoe나 CANdelaStudio와 대조해 검증한 것이 아니라,
> CANoe 샘플의 CDD와 CAN 로그, 이 저장소에 직접 만든 작은 CDD로 검증했습니다.

## 구성

| 경로 | 역할 |
|---|---|
| `crates/cdd-model` | CDD(XML) 파서, 3개 계층(인스턴스·템플릿·프로토콜) 해석, 메시지 레이아웃 IR, **인터프리터**(생성 코드 없이 바로 디코딩·인코딩) |
| `crates/cdd-codegen` | IR에서 Rust 코드를 만드는 생성기 (`quote`/`syn`/`prettyplease`) |
| `crates/cdd-rt` | 생성 코드가 쓰는 런타임. `no_std`. Reader/Writer, 테스터(`Tester::call`), 응답기(`Responder`), 루프백 시뮬레이션 |
| `crates/cdd-log` | `.asc`·`.blf` CAN 로그 읽기와 ISO-TP 재조립 |
| `apps/cddgen` | 명령줄 도구 |
| `testing/harness` | 생성 코드와 인터프리터를 비교하는 공용 검증 도구 |
| `testing/synthetic` | 직접 만든 작은 CDD(`mini-uds`, `mini-kwp`)와 그 테스트. Vector 파일이 필요 없음 |
| `testing/canoe-samples` | CANoe 샘플 CDD·로그로 하는 테스트. 샘플이 없으면 건너뜀 |
| `docs/` | 설계 검토 문서와 초기 프로토타입 |

## 사용법

```text
cddgen generate <cdd> <out_dir> [--ecu E] [--variant V] [--language L]... [--role tester|ecu|both]
                                [--strict] [--rt-crate NAME] [--default-nrc 0x12] [--check]
cddgen inspect  <cdd> [--service NAME]          # ECU, variant, 서비스 목록 / 한 서비스의 레이아웃
cddgen decode   <cdd> HEX... [--response]       # 페이로드가 어떤 서비스인지, 값은 무엇인지
cddgen decode-log <cdd> <log> [--id 700:600] [--channel N] [--brief]   # CAN 로그를 CANoe trace처럼 진단으로 해석
```

- `generate`는 `out_dir`에 `diag.rs`와 `manifest.json`(CDD의 SHA-256 포함)을 씁니다. `--check`는 쓰지 않고 최신인지만 확인하므로 CI에 넣을 수 있습니다.
- `--role tester`는 요청 송신과 응답 해석만, `--role ecu`는 시뮬레이션 ECU(`server`)만, `both`(기본값)는 둘 다 만듭니다.
- 생성할 수 없는 메시지는 건너뛰고 이유를 보고합니다. `--strict`면 하나라도 건너뛸 때 실패합니다.
- CDD에는 CAN ID가 없으므로 `decode-log`의 `--id 요청:응답`으로 지정합니다. 생략하면 트래픽에서 추정합니다.

### 생성된 코드를 쓰는 모습

```rust
// 테스터: 요청을 보내고 응답을 받는다.
let mut tester = Tester::new(my_transport);          // Transport 트레이트 구현
let mut rx = [0u8; 4095];
let reply = tester.call(&service::battery_read::Request, &mut rx)?;   // 0x78 response pending은 알아서 기다림
let battery = reply.expect_positive();
println!("{:?} V", battery.voltage.physical());

// 시뮬레이션 ECU: 서비스마다 on_<service> 메서드를 구현한다. 구현하지 않은 서비스는 NRC 0x12.
impl Handler for MyEcu {
    fn on_battery_read(&mut self, _: &battery_read::Request) -> Reply<battery_read::Response> { /* ... */ }
}
let mut tester = Tester::new(Loopback::new(Server::new(MyEcu::default())));   // 한 프로세스 안에서 연결
```

생성되는 것: 서비스마다 `service::<이름>::{Request, Response}`, 열거형·단위 변환 타입(`types`), 어느 서비스인지 알아내는
`AnyRequest`/`AnyResponse`(`decode`, `name`, `describe`, `encode`), `server::{Handler, Server}`, `nrc` 모듈, `SERVICES` 표.

## 설계에서 정한 것

- **서비스는 CDD의 세 계층에 흩어져 있습니다.** 인스턴스(DIAGINST)는 값을, 템플릿(DCLTMPL)은 연결을, 프로토콜(PROTOCOLSERVICE)은 바이트 레이아웃을 갖습니다.
  모두 `id` 참조로 이어지며, 해석기가 이를 하나의 레이아웃 IR로 합칩니다. 인터프리터와 생성기는 **같은 IR**을 읽습니다.
- **이름은 CAPL과 같습니다.** `SHORTCUTQUAL`, 없으면 `<DIAGINST>_<SERVICE>`.
- **suppress-positive-response 비트**는 `bool` 필드(`suppress_positive_response`)입니다. 서버는 이 비트가 켜진 요청의 긍정 응답을 보내지 않고, 부정 응답은 그대로 보냅니다.
- **MUX**: `MUXDT` 맨 위의 `STRUCTURE`는 어떤 `CASE`에도 선택값이 없을 때 쓰는 **기본 구조**입니다(ODX의 DEFAULT-CASE와 같은 의미). 생성 코드에서는 `Default` 변형입니다.
- **비트 컨테이너**: 컨테이너 안의 첫 멤버가 **가장 낮은 비트**를 씁니다.
- **개수 필드는 보통 필드입니다.** 개수 뒤에 목록이 오는 구조에서 개수가 목록 길이와 다르면 인코딩이 오류입니다. 일부러 틀린 개수를 보내려면 원시 바이트를 직접 보내세요.
- **BCD**: 생성 코드는 엄격합니다(자릿수 초과나 9를 넘는 니블은 오류). 인터프리터와 `decode-log`는 로그를 읽을 수 있도록 관대하게 원시 바이트와 주석으로 보여 줍니다. 이 차이는 의도한 것입니다.
- **텍스트 테이블**: 모든 항목이 값 하나면 `enum`(알 수 없는 값은 `Other(raw)`), 범위가 있으면 `label()`을 가진 newtype. 선형 변환(LINCOMP)은 `physical()`/`from_physical()`을 가진 newtype.
- **너무 짧은 메시지**(서비스를 가려낼 만큼의 상수 바이트가 없는 것)는 어느 서비스도 아닌 것으로 취급해 `on_unknown`(기본 NRC 0x11)으로 갑니다. 필요하면 `on_unknown`을 재정의하세요.

## 검증

1. **차분 테스트** (`testing/harness`): 서비스마다 인터프리터로 무작위 값을 인코딩한 뒤, 생성 코드가 같은 바이트를 받아들이고, 모든 값을 같게 해석하고(원시·기호·물리·단위), 같은 바이트로 다시 쓰는지 봅니다.
  자르거나 늘리거나 비트를 뒤집은 페이로드와 무작위 바이트에도 같은 판정을 내려야 합니다. 생성기의 일부를 일부러 틀리게 고쳐서 이 테스트가 실패하는 것도 확인했습니다(변이 검사).
2. **손으로 계산한 바이트** (`testing/synthetic/tests/wire.rs`): 차분 테스트는 "CDD를 잘못 읽은 것"을 못 잡으므로, 기능마다 바이트를 직접 계산해 둡니다.
3. **실제 CAN 로그** (`testing/canoe-samples`): CANoe 샘플의 KWP2000 로그를 ISO-TP로 재조립해 서비스와 값(BCD 식별번호, DTC 목록 등)이 맞게 나오는지 봅니다. 로그 읽기는 python-can과도 대조했습니다.
4. **시뮬레이션**: 테스터와 시뮬레이션 ECU를 한 프로세스에 연결해 요청·응답·NRC·suppress 비트·response pending을 확인합니다.

```text
cargo test --workspace                      # CANoe 샘플이 없으면 해당 테스트는 건너뜀
CANOE_SAMPLES="<...>\Sample Configurations 13.0.172\CAN"   # 샘플 위치(기본값은 Vector 설치 위치)
cargo clippy --workspace --all-targets -- -D warnings
```

`testing/synthetic/fixtures/*.cdd`는 `make_fixtures.py`가 만듭니다. 고치려면 스크립트를 수정하고 `python make_fixtures.py`로 다시 만드세요.

## 알려진 한계

- 데이터 타입: `VALTBL`, `COMPTBL`, `FORMULADT`는 지원하지 않습니다. 이를 쓰는 메시지는 건너뜁니다(보고에 이유가 나옴).
- 원소 크기가 가변인 반복 구조(`ListOfParameters`, 확장 데이터 레코드)와 `DOMAINDATAPROXYCOMP`(UDS 5.0.4)는 지원하지 않습니다.
- 로그: ASC의 CAN FD 프레임은 읽지 않고, BLF는 객체 타입 1·86·100·101만 읽습니다. ISO-TP는 normal addressing만 다룹니다.
- 생성 코드에는 타이밍(P2/P2*)과 CAN ID가 없습니다(CDD에 없음). 전송은 `Transport` 트레이트를 직접 구현해야 합니다.
- CANoe·CANdelaStudio와 직접 대조한 검증은 아직 하지 못했습니다.
