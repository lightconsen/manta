use super::*;

#[tokio::test]
#[serial]
async fn tool_shell_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40070,
        "Use ONLY the shell tool. Run: 'echo hello-from-shell-test' and report the exact output. Do not use any other tool.",
        "shell",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_file_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40080,
        "Use ONLY the file_write tool. Create /tmp/syscity-e2e-test.txt with content 'syscity-e2e-file-test'. \
         Do NOT call file_read or any other tool. Only call file_write once.",
        "file_write",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_todo_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40081,
        "Use ONLY the todo tool. Add a task 'e2e-todo-item' and list all todos. Do not use any other tool.",
        "todo",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_code_exec_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40082,
        "Use ONLY the execute_code tool. Run Python: print('syscity-code-exec-ok'). Report the output. Do not use any other tool.",
        "execute_code",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_web_fetch_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40083,
        "Use ONLY the web_fetch tool. Fetch https://example.com and tell me the page title. Do not use any other tool.",
        "web_fetch",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_memory_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40084,
        "Use ONLY the memory tool. Call action=store, content='Syscity is an AI agent framework'. Do not use any other tool.",
        "memory",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_glob_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40085,
        "Call ONLY the glob tool. Do NOT use shell. Pass pattern='src/**/*.rs' to list all .rs files.",
        "glob",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_grep_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40086,
        "Call ONLY the grep tool. Do NOT use shell. Pass pattern='pub fn', path='src' to search inside files.",
        "grep",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_process_invoked_via_chat() {
    let _results =
        run_tool_chat_test(40087, "Use ONLY the process tool. List running processes. Do not use any other tool.", "process")
            .await;
}

#[tokio::test]
#[serial]
async fn tool_nodes_invoked_via_chat() {
    let _results =
        run_tool_chat_test(40088, "Use ONLY the nodes tool. List available nodes. Do not use any other tool.", "nodes").await;
}

#[tokio::test]
#[serial]
async fn tool_web_search_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40089,
        "Use the web_search tool to search for Rust programming language. Do NOT use web_fetch.",
        "web_search",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_update_plan_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40090,
        "Use ONLY the update_plan tool. Create a plan titled 'Test Plan' with steps 'Step 1' and 'Step 2'. Do not use any other tool.",
        "update_plan",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_canvas_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40091,
        "Use ONLY the canvas tool. Present a canvas for session 'test-session-canvas' with a text component 'Hello Canvas'. Do not use any other tool.",
        "canvas",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_pdf_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40092,
        "Use ONLY the pdf tool. Generate a PDF with content 'Hello PDF' and save it to /tmp/syscity-e2e-test.pdf. Do not use any other tool.",
        "pdf",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_image_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40093,
        "Use ONLY the image tool. Get info about /tmp/syscity-test.png. Do not use any other tool.",
        "image",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_tts_invoked_via_chat() {
    let _results =
        run_tool_chat_test(40094, "Use ONLY the tts tool. Convert 'Hello' to speech. Do not use any other tool.", "tts")
            .await;
}

#[tokio::test]
#[serial]
async fn tool_memory_search_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40095,
        "Use ONLY the memory_search tool. Search for 'Syscity'. Do not use any other tool.",
        "memory_search",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_memory_get_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40096,
        "Use ONLY the memory_get tool. List all stored memories. Do not use any other tool.",
        "memory_get",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_cron_invoked_via_chat() {
    let _results =
        run_tool_chat_test(40097, "Use ONLY the cron tool. List all cron jobs. Do not use any other tool.", "cron").await;
}

#[tokio::test]
#[serial]
async fn tool_file_edit_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40098,
        "Call ONLY the file_edit tool. Do NOT use file_read. Pass file_path='/tmp/syscity-e2e-edit.txt', old_string='old text', new_string='new text'.",
        "file_edit",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_acp_spawn_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40099,
        "Use ONLY the acp_spawn tool. Spawn a subagent with task='say hello', mode='run'. Do not use any other tool.",
        "acp_spawn",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_acp_session_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40100,
        "Use ONLY the acp_session tool. List active ACP sessions with action=list. Do not use any other tool.",
        "acp_session",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_acp_session_kill_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40110,
        "Use ONLY the acp_session tool. Kill subagent 'test-subagent' with action=kill. Do not use any other tool.",
        "acp_session",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_sessions_list_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40101,
        "Use ONLY the sessions_list tool. List all active sessions. Do not use any other tool.",
        "sessions_list",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_sessions_history_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40102,
        "Use ONLY the sessions_history tool. Get history for session_id='test-session'. Do not use any other tool.",
        "sessions_history",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_sessions_send_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40103,
        "Use ONLY the sessions_send tool. Send 'ping' to session_id='test-session', subagent_id='test-subagent'. Do not use any other tool.",
        "sessions_send",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_sessions_yield_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40104,
        "Use ONLY the sessions_yield tool. Yield subagent 'test-subagent'. Do not use any other tool.",
        "sessions_yield",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_session_status_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40105,
        "Use ONLY the session_status tool. Get status for session_id='test-session'. Do not use any other tool.",
        "session_status",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_apply_patch_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40107,
        "Call ONLY the apply_patch tool. Do NOT use glob or any other tool. Pass patch='--- a.txt\\n+++ b.txt\\n@@ -1 +1 @@\\n-old\\n+new\\n'",
        "apply_patch",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_delegate_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40108,
        "Use ONLY the delegate tool. List delegated agents with action=list. Do not use any other tool.",
        "delegate",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_mcp_connection_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40109,
        "Use ONLY the mcp_connection tool. List all connected MCP servers. Do not use any other tool.",
        "mcp_connection",
    )
    .await;
}
