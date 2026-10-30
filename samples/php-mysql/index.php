<?php
function database(): PDO {
    $dsn = sprintf('mysql:host=%s;port=%s;dbname=%s;charset=utf8mb4',
        getenv('DB_HOST') ?: '127.0.0.1', getenv('DB_PORT') ?: '3306', getenv('DB_NAME'));
    return new PDO($dsn, getenv('DB_USER'), getenv('DB_PASSWORD'), [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_EMULATE_PREPARES => false,
    ]);
}
$path = parse_url($_SERVER['REQUEST_URI'], PHP_URL_PATH);
if ($path === '/health') {
    header('Content-Type: application/json');
    try {
        database()->query('SELECT 1');
        echo json_encode(['ok' => true, 'sample' => 'php-mysql', 'database' => getenv('DB_NAME')]);
    } catch (Throwable $error) {
        http_response_code(503);
        echo json_encode(['ok' => false, 'sample' => 'php-mysql', 'code' => $error->getCode()]);
    }
    exit;
}
if ($path === '/notes' && $_SERVER['REQUEST_METHOD'] === 'POST') {
    $pdo = database();
    $pdo->exec('CREATE TABLE IF NOT EXISTS notes (id INT AUTO_INCREMENT PRIMARY KEY, body VARCHAR(500) NOT NULL)');
    $statement = $pdo->prepare('INSERT INTO notes (body) VALUES (?)');
    $statement->execute([substr((string) ($_POST['body'] ?? ''), 0, 500)]);
    header('Content-Type: application/json');
    http_response_code(201);
    echo json_encode(['id' => (int) $pdo->lastInsertId()]);
    exit;
}
if (preg_match('#^/notes/(\d+)$#', $path, $match)) {
    $statement = database()->prepare('SELECT id, body FROM notes WHERE id = ?');
    $statement->execute([(int) $match[1]]);
    $note = $statement->fetch(PDO::FETCH_ASSOC);
    header('Content-Type: application/json');
    if (!$note) http_response_code(404);
    echo json_encode($note ?: ['found' => false]);
    exit;
}
?>
<!doctype html>
<html lang="en">
<meta charset="utf-8">
<title>Hostify PHP and MySQL sample</title>
<h1>Hostify PHP and MySQL sample</h1>
<p id="marker">hostify-sample-php-mysql</p>
</html>
