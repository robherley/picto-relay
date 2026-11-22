/** The DS's nine-square loading spinner, from ds.css. */
export function Loading() {
  return (
    <div className="loading-container" aria-hidden>
      {[1, 2, 3, 4, 5, 6, 7, 8, 9].map((square) => (
        <div key={square} className={`loading-${square}`} />
      ))}
    </div>
  );
}
