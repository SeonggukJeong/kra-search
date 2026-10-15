//! CPU 코어 정보. 임베딩 스레드 기본값 = min(4, 성능 코어 수)(DEC-050, results.md §5.2: Q8 4스레드가 최고의 97%,
//! 성능 코어 수(8)부터 느려지고 논리 스레드 전부는 7~12배 느림). 진단 도구도 같은 판별을 쓴다.

/// 물리 코어마다 EfficiencyClass 를 읽어 (가장 높은 등급 = 성능 코어 수, 나머지 = 효율 코어 수)를 돌려준다.
/// 등급이 하나뿐이면 하이브리드가 아니다(효율 0). Windows 에서만 잰다.
#[cfg(windows)]
pub fn core_split() -> Option<(usize, usize)> {
    use windows::Win32::System::SystemInformation::{
        GetLogicalProcessorInformationEx, RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
    };
    let mut len = 0u32;
    // 첫 호출은 필요한 크기만 알려 주고 실패한다(ERROR_INSUFFICIENT_BUFFER).
    let _ = unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, None, &mut len) };
    if len == 0 {
        return None;
    }
    // 구조체 정렬을 지키려고 u64 버퍼를 쓴다. 항목 크기(Size)는 공용체 전체 크기보다 작을 수 있어,
    // 마지막 항목을 구조체로 읽어도 버퍼 안에 있도록 구조체 하나만큼 더 잡는다.
    let entry = std::mem::size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>();
    let mut buf = vec![0u64; (len as usize + entry).div_ceil(8)];
    unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, Some(buf.as_mut_ptr().cast()), &mut len) }.ok()?;
    let base = buf.as_ptr().cast::<u8>();
    let mut classes = Vec::new();
    let mut off = 0usize;
    while off + 8 <= len as usize {
        let info = unsafe { &*base.add(off).cast::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>() };
        if info.Size == 0 {
            break;
        }
        if info.Relationship == RelationProcessorCore {
            classes.push(unsafe { info.Anonymous.Processor.EfficiencyClass });
        }
        off += info.Size as usize;
    }
    let top = *classes.iter().max()?;
    let perf = classes.iter().filter(|&&c| c == top).count();
    Some((perf, classes.len() - perf))
}

/// Windows 가 아니면 코어 종류를 재지 않는다.
#[cfg(not(windows))]
pub fn core_split() -> Option<(usize, usize)> {
    None
}

fn threads_for(perf: Option<usize>, logical: usize) -> usize {
    perf.unwrap_or(logical).clamp(1, 4)
}

/// 임베딩 스레드 기본값 = min(4, 성능 코어 수), 1~4.
/// Windows 는 `RelationProcessorCore` 항목, 곧 물리 코어를 센다(하이브리드가 아니면 모든 코어가 "성능" 코어).
/// Windows 가 아니거나 판별에 실패하면 논리 코어 수(`available_parallelism`)를 기준으로 한다.
pub fn default_embed_threads() -> usize {
    threads_for(core_split().map(|c| c.0), std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1))
}

