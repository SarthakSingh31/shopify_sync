DROP TABLE IF EXISTS LineItems;
DROP TABLE IF EXISTS Orders;
DROP TABLE IF EXISTS Stores;
DROP TABLE IF EXISTS AbandonedCheckout;

CREATE TABLE Stores(
    name TEXT PRIMARY KEY,
    access_token TEXT NOT NULL,
    last_abandoned_checkout_sync INTEGER
);

CREATE TABLE Orders(
    id REAL PRIMARY KEY,
    first_name TEXT,
    last_name TEXT,
    email TEXT,
    store_name TEXT NOT NULL,
    FOREIGN KEY (store_name)
        REFERENCES Stores (name)
            ON UPDATE CASCADE
            ON DELETE CASCADE
);

CREATE TABLE LineItems(
    title TEXT NOT NULL,
    order_id REAL NOT NULL,
    FOREIGN KEY (order_id)
        REFERENCES Orders (id)
            ON UPDATE CASCADE
            ON DELETE CASCADE
);

CREATE TABLE AbandonedCheckout(
    id INTEGER PRIMARY KEY,
    checkout_url TEXT NOT NULL,
    first_name TEXT,
    last_name TEXT,
    email TEXT,
    store_name TEXT NOT NULL,
    FOREIGN KEY (store_name)
        REFERENCES Stores (name)
            ON UPDATE CASCADE
            ON DELETE CASCADE
);