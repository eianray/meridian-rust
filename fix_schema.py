with open('/Users/malkobot/meridian-rust/meridian/src/routes/schema_infer.rs', 'r') as f:
    content = f.read()

old = '''    let python_out = tokio::task::spawn_blocking(move || {
        run_python_script(&tmp_path, mode)
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Join: {e}")))?;'''

new = '''    let python_out: String = tokio::task::spawn_blocking(move || {
        run_python_script(&tmp_path, mode)
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Join: {e})))
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Python script: {e}")))?;'''

if old in content:
    content = content.replace(old, new)
    with open('/Users/malkobot/meridian-rust/meridian/src/routes/schema_infer.rs', 'w') as f:
        f.write(content)
    print('replaced ok')
else:
    print('NOT FOUND')
    lines = content.split('\n')
    for i, l in enumerate(lines[213:220], 214):
        print(f'{i}: {repr(l)}')