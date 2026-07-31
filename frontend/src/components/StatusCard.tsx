interface StatusCardProps {
  title: string;
  status: string;
}

const StatusCard = ({ title, status }: StatusCardProps) => {
  const label = status.charAt(0).toUpperCase() + status.slice(1).toLowerCase();

  return (
    <div className="status-section">
      <div className="status-title">{title}</div>
      <div className="status-value">
        <span className={`status-text ${status.toLowerCase()}`}>{label}</span>
      </div>
    </div>
  );
};

export default StatusCard;
