use super::*;

#[tokio::test]
#[serial]
async fn tool_shell_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40070,
        "Use the shell tool to run the command 'echo hello-from-shell-test' and report the exact output.",
        "shell",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_file_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40080,
        "Use the file_write tool to create a file at /tmp/syscity-e2e-test.txt with content 'syscity-e2e-file-test'. \
         Then use file_read to read it back and confirm the content.",
        "file_write",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_todo_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40081,
        "Use the todo tool to add a task 'e2e-todo-item' and then list all todos.",
        "todo",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_code_exec_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40082,
        "Use the execute_code tool to run Python code that prints 'syscity-code-exec-ok' and report the output.",
        "execute_code",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_web_fetch_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40083,
        "Use the web_fetch tool to fetch https://example.com and tell me what the page title is.",
        "web_fetch",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_memory_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40084,
        "Call the memory tool with action=store, content='Syscity is an AI agent framework' to save a memory.",
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
        run_tool_chat_test(40087, "Use the process tool to list running processes.", "process")
            .await;
}

#[tokio::test]
#[serial]
async fn tool_nodes_invoked_via_chat() {
    let _results =
        run_tool_chat_test(40088, "Use the nodes tool to list available nodes.", "nodes").await;
}

#[tokio::test]
#[serial]
async fn tool_web_search_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40089,
        "Use the web_search tool to search for Rust programming language.",
        "web_search",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_update_plan_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40090,
        "Use the update_plan tool to create a plan titled 'Test Plan' with steps 'Step 1' and 'Step 2'.",
        "update_plan",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_canvas_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40091,
        "Use the canvas tool to present a canvas for session 'test-session-canvas' with a text component saying 'Hello Canvas'.",
        "canvas",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_pdf_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40092,
        "Use the pdf tool to generate a PDF with content 'Hello PDF' and save it.",
        "pdf",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_image_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40093,
        "Use the image tool to get info about the file /tmp/syscity-test.png.",
        "image",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_tts_invoked_via_chat() {
    let _results =
        run_tool_chat_test(40094, "Use the tts tool to convert the text 'Hello' to speech.", "tts")
            .await;
}

#[tokio::test]
#[serial]
async fn tool_memory_search_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40095,
        "Use the memory_search tool to search for 'Syscity'.",
        "memory_search",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_memory_get_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40096,
        "Use the memory_get tool to list all stored memories.",
        "memory_get",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_cron_invoked_via_chat() {
    let _results =
        run_tool_chat_test(40097, "Use the cron tool to list all cron jobs.", "cron").await;
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
        "Call the acp_spawn tool with task='say hello' and mode='run' to spawn a subagent.",
        "acp_spawn",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_acp_session_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40100,
        "Call the acp_session tool with action=list to list active ACP sessions.",
        "acp_session",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_acp_session_kill_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40110,
        "Call the acp_session tool with action=kill and subagent_id='test-subagent' to kill a subagent.",
        "acp_session",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_sessions_list_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40101,
        "Call the sessions_list tool to list all active sessions.",
        "sessions_list",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_sessions_history_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40102,
        "Call the sessions_history tool with session_id='test-session' to get chat history.",
        "sessions_history",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_sessions_send_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40103,
        "Call the sessions_send tool with session_id='test-session', subagent_id='test-subagent', message='ping' to send a message.",
        "sessions_send",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_sessions_yield_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40104,
        "Call the sessions_yield tool with subagent_id='test-subagent' to yield a subagent.",
        "sessions_yield",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_session_status_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40105,
        "Call the session_status tool with session_id='test-session' to get detailed session metadata.",
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
        "Call the delegate tool with action=list to list delegated child agents.",
        "delegate",
    )
    .await;
}

#[tokio::test]
#[serial]
async fn tool_mcp_connection_invoked_via_chat() {
    let _results = run_tool_chat_test(
        40109,
        "Use the mcp_connection tool to list all connected MCP servers.",
        "mcp_connection",
    )
    .await;
}
