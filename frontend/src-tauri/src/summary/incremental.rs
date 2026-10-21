use regex::Regex;
use serde_json::Value;
use sqlx::SqlitePool;

fn clean_line(line: &str) -> String {
    let time=Regex::new(r"^\s*(?:[-•]\s*)?\[\d{1,3}:\d{2}\]\s*").unwrap();
    let speaker=Regex::new(r"^(?:候选发言人|发言人)[^：:\s]{0,20}[：:]\s*").unwrap();
    speaker.replace(&time.replace(line,""),"").to_string()
}
fn normalized(text: &str) -> String {
    text.lines().flat_map(|line|clean_line(line).chars().filter(|c|c.is_alphanumeric()).collect::<Vec<_>>()).collect()
}
pub fn assemble(text: &str, rows: &[(i64,i64,i64,String)]) -> Option<String> {
    let source=normalized(text);let mut cursor=0;let mut latest=None;
    for (expected,(index,start,end,raw)) in rows.iter().enumerate() {
        if *index!=expected as i64 || *start!=cursor || *end<=*start {break;}
        let state:Value=match serde_json::from_str(raw){Ok(v)=>v,Err(_)=>break};
        if state["version"]!="adaptive-five-v1" {break;}
        let snapshot=state["source_text"].as_str().unwrap_or("");
        if snapshot.is_empty() || !source.contains(snapshot) || !state["structure"]["context_summary"].is_string(){break;}
        cursor=*end;latest=Some(state["structure"].clone());
    }
    let context=latest?;
    let timestamp=Regex::new(r"^\s*(?:[-•]\s*)?\[(\d{1,3}):(\d{2})\]").unwrap();
    let mut has_times=false;let mut tail=Vec::new();let mut keep=false;
    for line in text.lines() {
        if let Some(c)=timestamp.captures(line) {
            has_times=true;let seconds=c[1].parse::<i64>().unwrap_or(0)*60+c[2].parse::<i64>().unwrap_or(0);keep=seconds>=cursor;
        }
        if keep {tail.push(line);}
    }
    if !has_times {return None;}
    Some(format!("<incremental_summary>\n{}\n</incremental_summary>\n已有摘要覆盖至 {} 秒；以下为尚未归纳的原文。依据已有主题、决定和待办，整合尾段，按模板生成自然简洁的最终纪要。区分建议和决定；缺失负责人或日期写未指定。\n<remaining_transcript>\n{}\n</remaining_transcript>",context.to_string().replace('<',"\\u003c"),cursor,tail.join("\n")) + &format!("\n<full_transcript>\n{}\n</full_transcript>",text))
}
pub async fn context_for_meeting(pool: &SqlitePool, meeting: &str, text: &str) -> String {
    let mut waited=false;
    // The same GPU serializes these jobs anyway. Let the in-flight snapshot
    // become durable before assembling the final prompt, instead of repeating it.
    for _ in 0..2400 {
        if super::live_summary_commands::pending_summaries()==0 {break;}
        waited=true;tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    if waited {tokio::time::sleep(std::time::Duration::from_millis(500)).await;}
    let rows=sqlx::query_as::<_,(i64,i64,i64,String)>("SELECT minute_index,start_seconds,end_seconds,summary FROM meeting_minute_summaries WHERE meeting_id=? ORDER BY minute_index")
        .bind(meeting).fetch_all(pool).await.unwrap_or_default();
    assemble(text,&rows).unwrap_or_else(||text.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;use serde_json::json;
    fn row(index:i64,start:i64,end:i64,source:&str)->(i64,i64,i64,String){(index,start,end,json!({"version":"adaptive-five-v1","source_text":source,"structure":{"context_summary":"讨论了交付","todos":["核对方案"]}}).to_string())}
    #[test]fn consumes_saved_context_and_only_unsummarized_tail(){let text="[00:00] 发言人1：交付方案。\n[05:20] 发言人2：尾段确认。";let out=assemble(text,&[row(0,0,320,"交付方案")]).unwrap();assert!(out.contains("核对方案")&&out.contains("尾段确认"));assert!(out.contains("<full_transcript>") && out.contains("[00:00]"));}
    #[test]fn edited_or_discontinuous_sources_are_not_reused(){assert!(assemble("[00:00] 修改后的文字",&[row(0,0,320,"旧原文")]).is_none());assert!(assemble("[00:00] 原文",&[row(1,0,320,"原文")]).is_none());}
}
