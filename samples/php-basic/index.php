<?php
if (parse_url($_SERVER['REQUEST_URI'], PHP_URL_PATH) === '/health') {
    header('Content-Type: application/json');
    echo json_encode(['ok' => true, 'sample' => 'php', 'version' => PHP_VERSION, 'sapi' => PHP_SAPI]);
    exit;
}
?>
<!doctype html>
<html lang="en">
<meta charset="utf-8">
<title>Hostify PHP sample</title>
<h1>Hostify PHP sample</h1>
<p id="marker">hostify-sample-php</p>
<p>Running PHP <?= htmlspecialchars(PHP_VERSION) ?></p>
<p>SAMPLE_GREETING is <?= getenv('SAMPLE_GREETING') === false ? 'not set' : 'set' ?></p>
</html>
