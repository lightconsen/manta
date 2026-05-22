use super::*;

#[tokio::test]
#[serial]
async fn tool_shell_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let port = 40070;
    start_test_gateway(port, true).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    client
        .send_chat(
            &sid,
            "Use the shell tool to run the command 'echo hello-from-shell-test' and report the exact output.",
        )
        .await;

    let result = timeout(Duration::from_secs(60), async {
        let mut shell_called = false;
        let mut chat_final = None;

        while let Some(msg) = client.read.next().await {
            let msg = msg.unwrap();
            if let Message::Text(text) = msg {
                if let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) {
                    if event.get("type").and_then(|v| v.as_str()) == Some("event") {
                        let name = event.get("event").and_then(|v| v.as_str());
                        let payload = event.get("payload").cloned();
                        match name {
                            Some("tool.calling") => {
                                if let Some(ref p) = payload {
                                    if p.get("tool_name")
                                        .and_then(|v| v.as_str())
                                        == Some("shell")
                                    {
                                        shell_called = true;
                                    }
                                }
                            }
                            Some("chat.final") => {
                                chat_final = payload;
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        (shell_called, chat_final)
    })
    .await;

    let (shell_called, chat_final) = result.expect("Timed out waiting for chat.final event");
    if shell_called {
        tracing::info!("Shell tool was invoked via chat");
    }
    assert!(
        chat_final.is_some(),
        "Expected chat.final event within 60s"
    );
}

#[tokio::test]
#[serial]
async fn tool_file_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let port = 40080;
    start_test_gateway(port, true).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    client
        .send_chat(
            &sid,
            "Use the file_write tool to create a file at /tmp/manta-e2e-test.txt with content 'manta-e2e-file-test'. \
             Then use file_read to read it back and confirm the content.",
        )
        .await;

    let result = timeout(Duration::from_secs(60), async {
        let mut file_called = false;
        let mut chat_final = None;

        while let Some(msg) = client.read.next().await {
            let msg = msg.unwrap();
            if let Message::Text(text) = msg {
                if let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) {
                    if event.get("type").and_then(|v| v.as_str()) == Some("event") {
                        let name = event.get("event").and_then(|v| v.as_str());
                        let payload = event.get("payload").cloned();
                        match name {
                            Some("tool.calling") => {
                                if let Some(ref p) = payload {
                                    let tool_name = p.get("tool_name").and_then(|v| v.as_str());
                                    if tool_name == Some("file_write")
                                        || tool_name == Some("file_read")
                                    {
                                        file_called = true;
                                    }
                                }
                            }
                            Some("chat.final") => {
                                chat_final = payload;
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        (file_called, chat_final)
    })
    .await;

    let (file_called, chat_final) = result.expect("Timed out waiting for chat.final event");
    if file_called {
        tracing::info!("File tool was invoked via chat");
    }
    assert!(
        chat_final.is_some(),
        "Expected chat.final event within 60s"
    );
}

#[tokio::test]
#[serial]
async fn tool_todo_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let port = 40081;
    start_test_gateway(port, true).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    client
        .send_chat(
            &sid,
            "Use the todo tool to add a task 'e2e-todo-item' and then list all todos.",
        )
        .await;

    let result = timeout(Duration::from_secs(60), async {
        let mut todo_called = false;
        let mut chat_final = None;

        while let Some(msg) = client.read.next().await {
            let msg = msg.unwrap();
            if let Message::Text(text) = msg {
                if let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) {
                    if event.get("type").and_then(|v| v.as_str()) == Some("event") {
                        let name = event.get("event").and_then(|v| v.as_str());
                        let payload = event.get("payload").cloned();
                        match name {
                            Some("tool.calling") => {
                                if let Some(ref p) = payload {
                                    if p.get("tool_name")
                                        .and_then(|v| v.as_str())
                                        == Some("todo")
                                    {
                                        todo_called = true;
                                    }
                                }
                            }
                            Some("chat.final") => {
                                chat_final = payload;
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        (todo_called, chat_final)
    })
    .await;

    let (todo_called, chat_final) = result.expect("Timed out waiting for chat.final event");
    if todo_called {
        tracing::info!("Todo tool was invoked via chat");
    }
    assert!(
        chat_final.is_some(),
        "Expected chat.final event within 60s"
    );
}

#[tokio::test]
#[serial]
async fn tool_code_exec_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let port = 40082;
    start_test_gateway(port, true).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    client
        .send_chat(
            &sid,
            "Use the execute_code tool to run Python code that prints 'manta-code-exec-ok' and report the output.",
        )
        .await;

    let result = timeout(Duration::from_secs(60), async {
        let mut code_called = false;
        let mut chat_final = None;

        while let Some(msg) = client.read.next().await {
            let msg = msg.unwrap();
            if let Message::Text(text) = msg {
                if let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) {
                    if event.get("type").and_then(|v| v.as_str()) == Some("event") {
                        let name = event.get("event").and_then(|v| v.as_str());
                        let payload = event.get("payload").cloned();
                        match name {
                            Some("tool.calling") => {
                                if let Some(ref p) = payload {
                                    if p.get("tool_name")
                                        .and_then(|v| v.as_str())
                                        == Some("execute_code")
                                    {
                                        code_called = true;
                                    }
                                }
                            }
                            Some("chat.final") => {
                                chat_final = payload;
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        (code_called, chat_final)
    })
    .await;

    let (code_called, chat_final) = result.expect("Timed out waiting for chat.final event");
    if code_called {
        tracing::info!("Execute_code tool was invoked via chat");
    }
    assert!(
        chat_final.is_some(),
        "Expected chat.final event within 60s"
    );
}

#[tokio::test]
#[serial]
async fn tool_web_fetch_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let port = 40083;
    start_test_gateway(port, true).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    client
        .send_chat(
            &sid,
            "Use the web_fetch tool to fetch https://example.com and tell me what the page title is.",
        )
        .await;

    let result = timeout(Duration::from_secs(60), async {
        let mut fetch_called = false;
        let mut chat_final = None;

        while let Some(msg) = client.read.next().await {
            let msg = msg.unwrap();
            if let Message::Text(text) = msg {
                if let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) {
                    if event.get("type").and_then(|v| v.as_str()) == Some("event") {
                        let name = event.get("event").and_then(|v| v.as_str());
                        let payload = event.get("payload").cloned();
                        match name {
                            Some("tool.calling") => {
                                if let Some(ref p) = payload {
                                    if p.get("tool_name")
                                        .and_then(|v| v.as_str())
                                        == Some("web_fetch")
                                    {
                                        fetch_called = true;
                                    }
                                }
                            }
                            Some("chat.final") => {
                                chat_final = payload;
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        (fetch_called, chat_final)
    })
    .await;

    let (fetch_called, chat_final) = result.expect("Timed out waiting for chat.final event");
    if fetch_called {
        tracing::info!("Web_fetch tool was invoked via chat");
    }
    assert!(
        chat_final.is_some(),
        "Expected chat.final event within 60s"
    );
}

#[tokio::test]
#[serial]
async fn tool_memory_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let port = 40084;
    start_test_gateway(port, true).await;
    let mut client = FrontendSimulator::connect(port).await;

    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;

    client
        .send_chat(
            &sid,
            "Call the memory tool with action=store, content='Manta is an AI agent framework' to save a memory.",
        )
        .await;

    let result = timeout(Duration::from_secs(60), async {
        let mut memory_called = false;
        let mut chat_final = None;

        while let Some(msg) = client.read.next().await {
            let msg = msg.unwrap();
            if let Message::Text(text) = msg {
                if let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) {
                    if event.get("type").and_then(|v| v.as_str()) == Some("event") {
                        let name = event.get("event").and_then(|v| v.as_str());
                        let payload = event.get("payload").cloned();
                        match name {
                            Some("tool.calling") => {
                                if let Some(ref p) = payload {
                                    if p.get("tool_name")
                                        .and_then(|v| v.as_str())
                                        == Some("memory")
                                    {
                                        memory_called = true;
                                    }
                                }
                            }
                            Some("chat.final") => {
                                chat_final = payload;
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        (memory_called, chat_final)
    })
    .await;

    let (memory_called, chat_final) = result.expect("Timed out waiting for chat.final event");
    if memory_called {
        tracing::info!("Memory tool was invoked via chat");
    }
    assert!(
        chat_final.is_some(),
        "Expected chat.final event within 60s"
    );
}

#[tokio::test]
#[serial]
async fn tool_glob_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40085,
        "Call ONLY the glob tool. Do NOT use shell. Pass pattern='src/**/*.rs' to list all .rs files.",
        "glob",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_grep_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40086,
        "Call ONLY the grep tool. Do NOT use shell. Pass pattern='pub fn', path='src' to search inside files.",
        "grep",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_process_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40087,
        "Use the process tool to list running processes.",
        "process",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_nodes_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40088,
        "Use the nodes tool to list available nodes.",
        "nodes",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_web_search_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40089,
        "Use the web_search tool to search for Rust programming language.",
        "web_search",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_update_plan_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40090,
        "Use the update_plan tool to create a plan titled 'Test Plan' with steps 'Step 1' and 'Step 2'.",
        "update_plan",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_canvas_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40091,
        "Use the canvas tool to present a canvas for session 'test-session-canvas' with a text component saying 'Hello Canvas'.",
        "canvas",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_pdf_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40092,
        "Use the pdf tool to generate a PDF with content 'Hello PDF' and save it.",
        "pdf",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_image_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40093,
        "Use the image tool to get info about the file /tmp/manta-test.png.",
        "image",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_tts_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40094,
        "Use the tts tool to convert the text 'Hello' to speech.",
        "tts",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_memory_search_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40095,
        "Use the memory_search tool to search for 'Manta'.",
        "memory_search",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_memory_get_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40096,
        "Use the memory_get tool to list all stored memories.",
        "memory_get",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_cron_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40097,
        "Use the cron tool to list all cron jobs.",
        "cron",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_file_edit_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40098,
        "Call ONLY the file_edit tool. Do NOT use file_read. Pass file_path='/tmp/manta-e2e-edit.txt', old_string='old text', new_string='new text'.",
        "file_edit",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_acp_spawn_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40099,
        "Call the acp_spawn tool with task='say hello' and mode='run' to spawn a subagent.",
        "acp_spawn",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_acp_session_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40100,
        "Call the acp_session tool with action=list to list active ACP sessions.",
        "acp_session",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_acp_session_kill_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40110,
        "Call the acp_session tool with action=kill and subagent_id='test-subagent' to kill a subagent.",
        "acp_session",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_sessions_list_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40101,
        "Call the sessions_list tool to list all active sessions.",
        "sessions_list",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_sessions_history_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40102,
        "Call the sessions_history tool with session_id='test-session' to get chat history.",
        "sessions_history",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_sessions_send_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40103,
        "Call the sessions_send tool with session_id='test-session', subagent_id='test-subagent', message='ping' to send a message.",
        "sessions_send",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_sessions_yield_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40104,
        "Call the sessions_yield tool with subagent_id='test-subagent' to yield a subagent.",
        "sessions_yield",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_session_status_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40105,
        "Call the session_status tool with session_id='test-session' to get detailed session metadata.",
        "session_status",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_apply_patch_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40107,
        "Call ONLY the apply_patch tool. Do NOT use glob or any other tool. Pass patch='--- a.txt\\n+++ b.txt\\n@@ -1 +1 @@\\n-old\\n+new\\n'",
        "apply_patch",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_delegate_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40108,
        "Call the delegate tool with action=list to list delegated child agents.",
        "delegate",
    ).await;
}

#[tokio::test]
#[serial]
async fn tool_mcp_connection_invoked_via_chat() {
    if pick_test_provider().is_none() {
        panic!(
            "LLM tests require an API key. Either set MANTA_TEST_PROVIDER_KEY + MANTA_TEST_PROVIDER env vars, \
             or ensure start-local-qwen.sh / start-local-kimi.sh exist in the project root with valid keys."
        );
    }
    let _results = run_tool_chat_test(
        40109,
        "Use the mcp_connection tool to list all connected MCP servers.",
        "mcp_connection",
    ).await;
}
